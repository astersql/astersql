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

// MySQL DATE/DATETIME/TIMESTAMP/TIME（时长）类型核心实现，对齐 Go `types` 包。
//
// 以 `CoreTime` 位域打包年月日时分秒与微秒，低 4 位存放 FSP 与类型标记；
// 提供解析、格式化、打包、EXTRACT、区间运算，以及受 `TimeContext` 控制的
// 零日期/非法日期校验（对应 SQL Mode 相关行为）。

use crate::{CoreTime, mysql};
use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, LocalResult, NaiveDate, NaiveDateTime,
    TimeZone, Timelike, Utc, Weekday,
};
use chrono_tz::Tz;
use regex::Regex;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt::{self, Write};
use std::str::FromStr;
use std::sync::LazyLock;

/// 日期/时间显示布局常量（strftime 风格）。
pub const DateFormat: &str = "%Y-%m-%d";
pub const TimeFormat: &str = "%Y-%m-%d %H:%M:%S";
pub const TimeFSPFormat: &str = "%Y-%m-%d %H:%M:%S%.6f";
pub const UTCTimeFormat: &str = "%Y-%m-%d %H:%M:%S UTC";

/// YEAR 类型与时长边界相关常量。
pub const MinYear: i16 = 1901;
pub const MaxYear: i16 = 2155;
pub const MaxDuration: i64 = 8_385_959;
pub const TimeMaxHour: i32 = 838;
pub const TimeMaxMinute: i32 = 59;
pub const TimeMaxSecond: i32 = 59;
pub const TimeMaxValue: i32 = 8_385_959;
pub const TimeMaxValueSeconds: i64 = 3_020_399;
pub const MinTime: i64 = -TimeMaxValueSeconds * 1_000_000_000;
pub const MaxTime: i64 = TimeMaxValueSeconds * 1_000_000_000;
pub const ZeroDatetimeStr: &str = "0000-00-00 00:00:00";
pub const ZeroDateStr: &str = "0000-00-00";

/// 日期时间分量在切片中的下标，以及复合区间最大段数。
pub const YearIndex: usize = 0;
pub const MonthIndex: usize = 1;
pub const DayIndex: usize = 2;
pub const HourIndex: usize = 3;
pub const MinuteIndex: usize = 4;
pub const SecondIndex: usize = 5;
pub const MicrosecondIndex: usize = 6;
pub const TimeValueCnt: usize = 7;
pub const YearMonthMaxCnt: usize = 2;
pub const DayHourMaxCnt: usize = 2;
pub const DayMinuteMaxCnt: usize = 3;
pub const DaySecondMaxCnt: usize = 4;
pub const DayMicrosecondMaxCnt: usize = 5;
pub const HourMinuteMaxCnt: usize = 2;
pub const HourSecondMaxCnt: usize = 3;
pub const HourMicrosecondMaxCnt: usize = 4;
pub const MinuteSecondMaxCnt: usize = 2;
pub const MinuteMicrosecondMaxCnt: usize = 3;
pub const SecondMicrosecondMaxCnt: usize = 2;

/// Go `time.Duration` 一天/一周纳秒数，以及 FSP（小数秒精度）范围。
pub const GoDurationDay: i64 = 86_400_000_000_000;
pub const GoDurationWeek: i64 = GoDurationDay * 7;
pub const UnspecifiedFsp: i32 = -1;
pub const MinFsp: i32 = 0;
pub const MaxFsp: i32 = 6;
pub const DefaultFsp: i32 = 0;

/// CoreTime 位域偏移/宽度，以及低 4 位 FSP+类型标记掩码。
pub const yearBitFieldOffset: u64 = 50;
pub const yearBitFieldWidth: u64 = 14;
pub const monthBitFieldOffset: u64 = 46;
pub const monthBitFieldWidth: u64 = 4;
pub const dayBitFieldOffset: u64 = 41;
pub const dayBitFieldWidth: u64 = 5;
pub const hourBitFieldOffset: u64 = 36;
pub const hourBitFieldWidth: u64 = 5;
pub const minuteBitFieldOffset: u64 = 30;
pub const minuteBitFieldWidth: u64 = 6;
pub const secondBitFieldOffset: u64 = 24;
pub const secondBitFieldWidth: u64 = 6;
pub const microsecondBitFieldOffset: u64 = 4;
pub const microsecondBitFieldWidth: u64 = 20;
pub const fspTtBitFieldMask: u64 = 0b1111;
pub const fspBitFieldMask: u64 = 0b1110;
pub const coreTimeBitFieldMask: u64 = !fspTtBitFieldMask;
pub const fspTtForDate: u64 = 0b1110;

#[derive(Debug, Clone, Eq, PartialEq)]
/// 时间解析/运算错误。
pub struct TimeError(pub String);

impl TimeError {
    fn wrong(kind: &str, value: impl fmt::Display) -> Self {
        Self(format!("incorrect {kind} value: {value}"))
    }

    fn overflow(value: impl fmt::Display) -> Self {
        Self(format!("datetime function overflow: {value}"))
    }
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TimeError {}

/// 解析上下文：SQL Mode 风格标志、时区与告警回调。
pub trait TimeContext {
    fn flags(&self) -> TimeFlags {
        TimeFlags::default()
    }
    fn location(&self) -> Tz {
        chrono_tz::UTC
    }
    fn append_warning(&self, _warning: TimeError) {}
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 控制零日期、非法日期等校验是否放宽的标志集合。
pub struct TimeFlags {
    pub ignore_zero_in_date: bool,
    pub ignore_invalid_date: bool,
    pub ignore_zero_date: bool,
    pub cast_time_to_year_through_concat: bool,
}

#[derive(Clone, Copy, Debug, Default)]
/// 严格上下文：默认拒绝零/非法日期。
pub struct StrictTimeContext;
impl TimeContext for StrictTimeContext {}
/// 全局严格上下文单例。
pub const StrictContext: StrictTimeContext = StrictTimeContext;

#[derive(Clone, Copy, Debug)]
/// 可配置标志与时区的基础上下文。
pub struct BasicTimeContext {
    pub flags: TimeFlags,
    pub location: Tz,
}

impl Default for BasicTimeContext {
    fn default() -> Self {
        Self {
            flags: TimeFlags::default(),
            location: chrono_tz::UTC,
        }
    }
}

impl TimeContext for BasicTimeContext {
    fn flags(&self) -> TimeFlags {
        self.flags
    }
    fn location(&self) -> Tz {
        self.location
    }
}

/// 规范化 FSP：未指定用默认值，负值报错，超过 MaxFsp 截断。
pub fn CheckFsp(fsp: i32) -> Result<i32, TimeError> {
    if fsp == UnspecifiedFsp {
        return Ok(DefaultFsp);
    }
    if fsp < MinFsp {
        return Err(TimeError(format!("invalid fsp {fsp}")));
    }
    Ok(fsp.min(MaxFsp))
}

/// 将年月日时分秒微秒打包为 CoreTime（不做范围检查）。
pub fn FromDate(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
) -> CoreTime {
    let mut value = 0_u64;
    value |= ((microsecond as u64) << microsecondBitFieldOffset)
        & (((1 << microsecondBitFieldWidth) - 1) << microsecondBitFieldOffset);
    value |= ((second as u64) << secondBitFieldOffset)
        & (((1 << secondBitFieldWidth) - 1) << secondBitFieldOffset);
    value |= ((minute as u64) << minuteBitFieldOffset)
        & (((1 << minuteBitFieldWidth) - 1) << minuteBitFieldOffset);
    value |= ((hour as u64) << hourBitFieldOffset)
        & (((1 << hourBitFieldWidth) - 1) << hourBitFieldOffset);
    value |=
        ((day as u64) << dayBitFieldOffset) & (((1 << dayBitFieldWidth) - 1) << dayBitFieldOffset);
    value |= ((month as u64) << monthBitFieldOffset)
        & (((1 << monthBitFieldWidth) - 1) << monthBitFieldOffset);
    value |= ((year as u64) << yearBitFieldOffset)
        & (((1 << yearBitFieldWidth) - 1) << yearBitFieldOffset);
    CoreTime(value)
}

/// 打包 CoreTime 并返回各分量是否落在位域可表示范围内。
pub fn FromDateChecked(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
) -> (CoreTime, bool) {
    let values = [year, month, day, hour, minute, second, microsecond];
    let widths = [14, 4, 5, 5, 6, 6, 20];
    if values
        .iter()
        .zip(widths)
        .any(|(&v, w)| v < 0 || (v as u64) >= (1 << w))
    {
        return (CoreTime(0), false);
    }
    (
        FromDate(year, month, day, hour, minute, second, microsecond),
        true,
    )
}

/// 从 chrono DateTime 提取分量并打包。
pub fn FromGoTime(t: DateTime<Tz>) -> CoreTime {
    let rounded = t + ChronoDuration::nanoseconds(500);
    FromDate(
        rounded.year(),
        rounded.month() as i32,
        rounded.day() as i32,
        rounded.hour() as i32,
        rounded.minute() as i32,
        rounded.second() as i32,
        rounded.nanosecond() as i32 / 1_000,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// MySQL 时间值：CoreTime 高位为日历字段，低 4 位为 FSP/类型。
pub struct Time {
    pub coreTime: CoreTime,
}

impl Default for Time {
    fn default() -> Self {
        Self {
            coreTime: CoreTime(0),
        }
    }
}

/// 全零时间（0000-00-00 00:00:00）。
pub const ZeroTime: Time = Time {
    coreTime: CoreTime(0),
};
/// 零时长。
pub const ZeroDuration: Duration = Duration {
    Duration: 0,
    Fsp: DefaultFsp,
};

/// 用类型与 FSP 写入低 4 位标记，构造 Time。
pub fn NewTime(core_time: CoreTime, tp: u8, fsp: i32) -> Time {
    let mut raw = core_time.0 & coreTimeBitFieldMask;
    if tp == mysql::TypeDate {
        raw |= fspTtForDate;
    } else {
        let fsp = if fsp == UnspecifiedFsp {
            DefaultFsp
        } else {
            fsp.clamp(0, 6)
        };
        raw |= (fsp as u64) << 1;
        if tp == mysql::TypeTimestamp {
            raw |= 1;
        }
    }
    Time {
        coreTime: CoreTime(raw),
    }
}

/// DATETIME 合法下界 0001-01-01。
pub fn MinDatetime() -> Time {
    NewTime(FromDate(1, 1, 1, 0, 0, 0, 0), mysql::TypeDatetime, 0)
}
/// DATETIME 合法上界 9999-12-31 23:59:59.999999。
pub fn MaxDatetime() -> Time {
    NewTime(
        FromDate(9999, 12, 31, 23, 59, 59, 999_999),
        mysql::TypeDatetime,
        6,
    )
}
/// TIMESTAMP 下界（UTC）1970-01-01 00:00:01。
pub fn MinTimestamp() -> Time {
    NewTime(FromDate(1970, 1, 1, 0, 0, 1, 0), mysql::TypeTimestamp, 0)
}
/// TIMESTAMP 上界（UTC）2038-01-19 03:14:07.999999。
pub fn MaxTimestamp() -> Time {
    NewTime(
        FromDate(2038, 1, 19, 3, 14, 7, 999_999),
        mysql::TypeTimestamp,
        6,
    )
}

/// 供包外测试复用的 TIMESTAMP 范围与时区校验。
/// CheckTimestampTypeForTest exposes the timestamp range and timezone validation
/// used by Go's package-external tests.
pub fn CheckTimestampTypeForTest(core_time: CoreTime, location: Tz) -> Result<(), TimeError> {
    if core_time == CoreTime(0) {
        return Ok(());
    }

    let original = NewTime(core_time, mysql::TypeTimestamp, DefaultFsp);
    let mut bounded = original;
    if location != chrono_tz::UTC {
        bounded.ConvertTimeZone(location, chrono_tz::UTC)?;
    }
    if bounded.Compare(MaxTimestamp()) > 0 || bounded.Compare(MinTimestamp()) < 0 {
        return Err(TimeError::wrong("timestamp", original.String()));
    }

    original.GoTime(location)?;
    Ok(())
}

impl Time {
    fn getFspTt(self) -> u64 {
        self.coreTime.0 & fspTtBitFieldMask
    }
    fn setFspTt(&mut self, fsp_tt: u64) {
        self.coreTime.0 = (self.coreTime.0 & !fspTtBitFieldMask) | (fsp_tt & fspTtBitFieldMask);
    }

    /// 从低位标记解码 MySQL 类型（Date/Datetime/Timestamp）。
    pub fn Type(self) -> u8 {
        if self.getFspTt() == fspTtForDate {
            mysql::TypeDate
        } else if self.coreTime.0 & 1 == 1 {
            mysql::TypeTimestamp
        } else {
            mysql::TypeDatetime
        }
    }

    /// 读取小数秒精度；DATE 固定为 0。
    pub fn Fsp(self) -> i32 {
        if self.getFspTt() == fspTtForDate {
            0
        } else {
            (self.getFspTt() >> 1) as i32
        }
    }

    /// 更新类型标记，必要时重置 DATE 专用位模式。
    pub fn SetType(&mut self, tp: u8) {
        let mut bits = self.getFspTt();
        if bits == fspTtForDate && tp != mysql::TypeDate {
            bits = 0;
        }
        match tp {
            mysql::TypeDate => bits = fspTtForDate,
            mysql::TypeTimestamp => bits |= 1,
            mysql::TypeDatetime => bits &= !1,
            _ => return,
        }
        self.setFspTt(bits);
    }

    /// 更新 FSP；DATE 类型忽略。
    pub fn SetFsp(&mut self, fsp: i32) {
        if self.Type() == mysql::TypeDate {
            return;
        }
        let fsp = if fsp == UnspecifiedFsp {
            DefaultFsp
        } else {
            fsp.clamp(0, 6)
        };
        self.coreTime.0 = (self.coreTime.0 & !fspBitFieldMask) | ((fsp as u64) << 1);
    }

    /// 取出清除低 4 位标记后的日历位域。
    pub fn CoreTime(self) -> CoreTime {
        CoreTime(self.coreTime.0 & coreTimeBitFieldMask)
    }
    /// 写入日历位域，保留原有 FSP/类型标记。
    pub fn SetCoreTime(&mut self, ct: CoreTime) {
        self.coreTime.0 = (self.coreTime.0 & fspTtBitFieldMask) | (ct.0 & coreTimeBitFieldMask);
    }
    /// 年份分量。
    pub fn Year(self) -> i32 {
        self.CoreTime().Year()
    }
    /// 月份分量。
    pub fn Month(self) -> i32 {
        self.CoreTime().Month()
    }
    /// 日分量。
    pub fn Day(self) -> i32 {
        self.CoreTime().Day()
    }
    /// 小时分量。
    pub fn Hour(self) -> i32 {
        self.CoreTime().Hour()
    }
    /// 分钟分量。
    pub fn Minute(self) -> i32 {
        self.CoreTime().Minute()
    }
    /// 秒分量。
    pub fn Second(self) -> i32 {
        self.CoreTime().Second()
    }
    /// 微秒分量。
    pub fn Microsecond(self) -> i32 {
        self.CoreTime().Microsecond()
    }
    /// 返回 (时, 分, 秒)。
    pub fn Clock(self) -> (i32, i32, i32) {
        (self.Hour(), self.Minute(), self.Second())
    }
    /// CoreTime 位域是否全零。
    pub fn IsZero(self) -> bool {
        self.CoreTime().0 == 0
    }
    /// 月或日为 0（零日期的部分零）。
    pub fn InvalidZero(self) -> bool {
        self.Month() == 0 || self.Day() == 0
    }

    fn naive(self) -> Result<NaiveDateTime, TimeError> {
        let date = NaiveDate::from_ymd_opt(self.Year(), self.Month() as u32, self.Day() as u32)
            .ok_or_else(|| TimeError::wrong("datetime", self.String()))?;
        date.and_hms_micro_opt(
            self.Hour() as u32,
            self.Minute() as u32,
            self.Second() as u32,
            self.Microsecond() as u32,
        )
        .ok_or_else(|| TimeError::wrong("datetime", self.String()))
    }

    /// 转为带时区的 chrono 时间；DST 空洞报错，歧义取较早者。
    pub fn GoTime(self, loc: Tz) -> Result<DateTime<Tz>, TimeError> {
        match loc.from_local_datetime(&self.naive()?) {
            LocalResult::Single(t) => Ok(t),
            LocalResult::Ambiguous(a, _) => Ok(a),
            LocalResult::None => Err(TimeError::wrong("datetime", self.String())),
        }
    }

    /// DST 空洞时向前搜索最多 240 分钟找到合法本地时间。
    pub fn AdjustedGoTime(self, loc: Tz) -> Result<DateTime<Tz>, TimeError> {
        let naive = self.naive()?;
        match loc.from_local_datetime(&naive) {
            LocalResult::Single(t) => Ok(t),
            LocalResult::Ambiguous(a, _) => Ok(a),
            LocalResult::None => {
                for minutes in 1..=240 {
                    if let LocalResult::Single(t) =
                        loc.from_local_datetime(&(naive + ChronoDuration::minutes(minutes)))
                    {
                        return Ok(t);
                    }
                }
                Err(TimeError::wrong("datetime", self.String()))
            }
        }
    }

    /// 在两个时区之间转换并写回日历字段。
    pub fn ConvertTimeZone(&mut self, from: Tz, to: Tz) -> Result<(), TimeError> {
        if self.IsZero() {
            return Ok(());
        }
        let converted = self.GoTime(from)?.with_timezone(&to);
        self.SetCoreTime(FromGoTime(converted));
        Ok(())
    }

    /// 按类型与 FSP 格式化为 MySQL 文本。
    pub fn String(self) -> String {
        if self.Type() == mysql::TypeDate {
            return format!("{:04}-{:02}-{:02}", self.Year(), self.Month(), self.Day());
        }
        let mut out = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.Year(),
            self.Month(),
            self.Day(),
            self.Hour(),
            self.Minute(),
            self.Second()
        );
        if self.Fsp() > 0 {
            let frac = format!("{:06}", self.Microsecond());
            out.push('.');
            out.push_str(&frac[..self.Fsp() as usize]);
        }
        out
    }

    /// 转为 DECIMAL 数值表示。
    pub fn ToNumber(self) -> Decimal {
        if self.IsZero() {
            return Decimal::ZERO;
        }
        Decimal::from_str(&self.String().replace(['-', ':', ' '], "")).unwrap_or(Decimal::ZERO)
    }

    /// 就地填入 DECIMAL。
    pub fn FillNumber(self, dec: &mut Decimal) {
        *dec = self.ToNumber();
    }

    /// 转换到另一 MySQL 时间类型并校验。
    pub fn Convert<C: TimeContext>(self, ctx: &C, tp: u8) -> Result<Time, TimeError> {
        let mut result = self;
        result.SetType(tp);
        if !result.IsZero() {
            result.Check(ctx)?;
        }
        Ok(result)
    }

    /// 提取时间部分为 Duration。
    pub fn ConvertToDuration(self) -> Result<Duration, TimeError> {
        if self.IsZero() {
            return Ok(ZeroDuration);
        }
        Ok(Duration::from_parts(
            self.Hour(),
            self.Minute(),
            self.Second(),
            self.Microsecond(),
            self.Fsp(),
        ))
    }

    /// 比较两个 Time，返回 -1/0/1。
    pub fn Compare(self, other: Time) -> i32 {
        match self.CoreTime().0.cmp(&other.CoreTime().0) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    /// 与字符串解析出的时间比较。
    pub fn CompareString<C: TimeContext>(self, ctx: &C, value: &str) -> Result<i32, TimeError> {
        Ok(self.Compare(ParseTime(ctx, value, self.Type(), MaxFsp)?))
    }

    /// 按目标 FSP 四舍五入小数秒。
    pub fn RoundFrac<C: TimeContext>(self, ctx: &C, fsp: i32) -> Result<Time, TimeError> {
        if self.Type() == mysql::TypeDate || self.IsZero() {
            return Ok(self);
        }
        let fsp = CheckFsp(fsp)?;
        if fsp == self.Fsp() {
            return Ok(self);
        }
        let unit = 10_i64.pow((6 - fsp) as u32);
        let rounded = ((self.Microsecond() as i64 + unit / 2) / unit) * unit;
        let (mut naive, valid_date) = match self.naive() {
            Ok(value) => (value, true),
            Err(_) => NaiveDate::from_ymd_opt(1, 1, 1)
                .unwrap()
                .and_hms_micro_opt(
                    self.Hour() as u32,
                    self.Minute() as u32,
                    self.Second() as u32,
                    0,
                )
                .map(|value| (value, false))
                .ok_or_else(|| TimeError::wrong("time", self.String()))?,
        };
        naive = naive.with_nanosecond(0).unwrap() + ChronoDuration::microseconds(rounded);
        if !valid_date && naive.day() != 1 {
            return Err(TimeError::wrong("time", self.String()));
        }
        let mut result = NewTime(
            FromDate(
                if valid_date {
                    naive.year()
                } else {
                    self.Year()
                },
                if valid_date {
                    naive.month() as i32
                } else {
                    self.Month()
                },
                if valid_date {
                    naive.day() as i32
                } else {
                    self.Day()
                },
                naive.hour() as i32,
                naive.minute() as i32,
                naive.second() as i32,
                naive.nanosecond() as i32 / 1_000,
            ),
            self.Type(),
            fsp,
        );
        result.Check(ctx)?;
        result.SetFsp(fsp);
        Ok(result)
    }

    /// 打包为 TiDB/MySQL 存储用的紧凑 u64。
    pub fn ToPackedUint(self) -> Result<u64, TimeError> {
        if self.IsZero() {
            return Ok(0);
        }
        let ymd = (((self.Year() * 13 + self.Month()) << 5) | self.Day()) as u64;
        let hms = ((self.Hour() << 12) | (self.Minute() << 6) | self.Second()) as u64;
        Ok(((ymd << 17 | hms) << 24) | self.Microsecond() as u64)
    }

    /// 从紧凑 u64 解包写回自身。
    pub fn FromPackedUint(&mut self, packed: u64) -> Result<(), TimeError> {
        if packed == 0 {
            self.SetCoreTime(CoreTime(0));
            return Ok(());
        }
        let ymdhms = packed >> 24;
        let ymd = ymdhms >> 17;
        let day = (ymd & 31) as i32;
        let ym = ymd >> 5;
        let month = (ym % 13) as i32;
        let year = (ym / 13) as i32;
        let hms = ymdhms & 0x1ffff;
        let second = (hms & 63) as i32;
        let minute = ((hms >> 6) & 63) as i32;
        let hour = (hms >> 12) as i32;
        self.SetCoreTime(FromDate(
            year,
            month,
            day,
            hour,
            minute,
            second,
            (packed & 0xffffff) as i32,
        ));
        Ok(())
    }

    /// 按上下文标志校验零日期、非法日期与 TIMESTAMP 范围。
    pub fn Check<C: TimeContext>(self, ctx: &C) -> Result<(), TimeError> {
        if self.IsZero() {
            return Ok(());
        }
        if self.InvalidZero() {
            return if ctx.flags().ignore_zero_in_date {
                Ok(())
            } else {
                Err(TimeError::wrong("date", self.String()))
            };
        }
        if self.Year() < 0
            || self.Year() > 9999
            || self.Hour() > 23
            || self.Minute() > 59
            || self.Second() > 59
            || self.Microsecond() > 999_999
        {
            return Err(TimeError::wrong("datetime", self.String()));
        }
        if !ctx.flags().ignore_invalid_date
            && NaiveDate::from_ymd_opt(self.Year(), self.Month() as u32, self.Day() as u32)
                .is_none()
        {
            return Err(TimeError::wrong("date", self.String()));
        }
        if self.Type() == mysql::TypeTimestamp {
            let value = self.AdjustedGoTime(ctx.location())?.with_timezone(&Utc);
            let min = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 1).unwrap();
            let max = Utc.with_ymd_and_hms(2038, 1, 19, 3, 14, 7).unwrap()
                + ChronoDuration::microseconds(999_999);
            if value < min || value > max {
                return Err(TimeError::wrong("timestamp", self.String()));
            }
        }
        Ok(())
    }

    /// 两时间之差，结果为 Duration。
    pub fn Sub<C: TimeContext>(self, ctx: &C, other: Time) -> Duration {
        let micros = if self.Type() == mysql::TypeTimestamp && other.Type() == mysql::TypeTimestamp
        {
            match (self.GoTime(ctx.location()), other.GoTime(ctx.location())) {
                (Ok(a), Ok(b)) => a.signed_duration_since(b).num_microseconds().unwrap_or(0),
                _ => 0,
            }
        } else {
            datetime_micros(self.CoreTime()) - datetime_micros(other.CoreTime())
        };
        Duration {
            Duration: micros * 1_000,
            Fsp: self.Fsp().max(other.Fsp()),
        }
    }

    /// 加上 Duration，必要时进位到日期。
    pub fn Add<C: TimeContext>(self, ctx: &C, duration: Duration) -> Result<Time, TimeError> {
        let base = self.naive()?;
        let added = base
            .checked_add_signed(ChronoDuration::nanoseconds(duration.Duration))
            .ok_or_else(|| TimeError::overflow(self.String()))?;
        let mut result = NewTime(
            FromDate(
                added.year(),
                added.month() as i32,
                added.day() as i32,
                added.hour() as i32,
                added.minute() as i32,
                added.second() as i32,
                added.nanosecond() as i32 / 1_000,
            ),
            self.Type(),
            self.Fsp().max(duration.Fsp),
        );
        if self.Type() == mysql::TypeDate {
            result.SetCoreTime(FromDate(
                added.year(),
                added.month() as i32,
                added.day() as i32,
                0,
                0,
                0,
                0,
            ));
        }
        result.Check(ctx)?;
        Ok(result)
    }

    /// MySQL DATE_FORMAT 风格格式化。
    pub fn DateFormat(self, layout: &str) -> Result<String, TimeError> {
        let mut output = String::new();
        let mut chars = layout.chars();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                output.push(ch);
                continue;
            }
            let token = chars
                .next()
                .ok_or_else(|| TimeError("trailing % in date format".into()))?;
            match token {
                '%' => output.push('%'),
                'b' => output
                    .push_str(&MONTH_ABBREV[(self.Month().saturating_sub(1) as usize).min(11)]),
                'M' => {
                    output.push_str(&MonthNames[(self.Month().saturating_sub(1) as usize).min(11)])
                }
                'm' => write!(output, "{:02}", self.Month()).unwrap(),
                'c' => write!(output, "{}", self.Month()).unwrap(),
                'D' => output.push_str(&abbrDayOfMonth(self.Day())),
                'd' => write!(output, "{:02}", self.Day()).unwrap(),
                'e' => write!(output, "{}", self.Day()).unwrap(),
                'j' => write!(output, "{:03}", year_day(self.CoreTime())).unwrap(),
                'H' => write!(output, "{:02}", self.Hour()).unwrap(),
                'k' => write!(output, "{}", self.Hour()).unwrap(),
                'h' | 'I' => write!(output, "{:02}", hour12(self.Hour())).unwrap(),
                'l' => write!(output, "{}", hour12(self.Hour())).unwrap(),
                'i' => write!(output, "{:02}", self.Minute()).unwrap(),
                'p' => output.push_str(if self.Hour() < 12 { "AM" } else { "PM" }),
                'r' => write!(
                    output,
                    "{:02}:{:02}:{:02} {}",
                    hour12(self.Hour()),
                    self.Minute(),
                    self.Second(),
                    if self.Hour() < 12 { "AM" } else { "PM" }
                )
                .unwrap(),
                'T' => write!(
                    output,
                    "{:02}:{:02}:{:02}",
                    self.Hour(),
                    self.Minute(),
                    self.Second()
                )
                .unwrap(),
                'S' | 's' => write!(output, "{:02}", self.Second()).unwrap(),
                'f' => write!(output, "{:06}", self.Microsecond()).unwrap(),
                'U' => write!(output, "{:02}", mysql_week(self.CoreTime(), 0)).unwrap(),
                'u' => write!(output, "{:02}", mysql_week(self.CoreTime(), 1)).unwrap(),
                'V' => write!(output, "{:02}", mysql_week(self.CoreTime(), 2)).unwrap(),
                'v' => write!(output, "{:02}", mysql_year_week(self.CoreTime(), 3).1).unwrap(),
                'a' => output.push_str(&ABBREV_WEEKDAY[weekday_index(self.CoreTime())]),
                'W' => output.push_str(&WEEKDAY_NAMES[weekday_index(self.CoreTime())]),
                'w' => write!(output, "{}", (weekday_index(self.CoreTime()) + 1) % 7).unwrap(),
                'Y' => write!(output, "{:04}", self.Year()).unwrap(),
                'y' => write!(output, "{:02}", self.Year().rem_euclid(100)).unwrap(),
                'X' => output.push_str(&format_mysql_week_year(
                    mysql_year_week(self.CoreTime(), 2).0,
                )),
                'x' => output.push_str(&format_mysql_week_year(
                    mysql_year_week(self.CoreTime(), 3).0,
                )),
                other => output.push(other),
            }
        }
        Ok(output)
    }

    /// STR_TO_DATE：按 format 解析并写回；失败返回 false。
    pub fn StrToDate<C: TimeContext>(&mut self, ctx: &C, date: &str, format: &str) -> bool {
        match parse_str_to_date(date, format) {
            Ok((core, warning)) => {
                *self = NewTime(core, mysql::TypeDatetime, GetFsp(date));
                if self.Check(ctx).is_err() {
                    return false;
                }
                if warning {
                    ctx.append_warning(TimeError("truncated wrong value".into()));
                }
                true
            }
            Err(_) => {
                *self = NewTime(CoreTime(0), mysql::TypeDatetime, 0);
                false
            }
        }
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}

/// 取当前 UTC 时间并包装为指定类型。
pub fn CurrentTime(tp: u8) -> Time {
    NewTime(FromGoTime(Utc::now().with_timezone(&chrono_tz::UTC)), tp, 0)
}

/// TIMESTAMPDIFF：按 unit 计算 t2-t1 的整段差值。
pub fn TimestampDiff(unit: &str, t1: Time, t2: Time) -> i64 {
    let neg = t2.Compare(t1) < 0;
    let (beg, end) = if neg { (t2, t1) } else { (t1, t2) };
    let sign = if neg { -1 } else { 1 };
    if matches!(unit, "YEAR" | "QUARTER" | "MONTH") {
        let mut months = (end.Year() - beg.Year()) as i64 * 12 + (end.Month() - beg.Month()) as i64;
        if (
            end.Day(),
            end.Hour(),
            end.Minute(),
            end.Second(),
            end.Microsecond(),
        ) < (
            beg.Day(),
            beg.Hour(),
            beg.Minute(),
            beg.Second(),
            beg.Microsecond(),
        ) {
            months -= 1;
        }
        return sign
            * match unit {
                "YEAR" => months / 12,
                "QUARTER" => months / 3,
                _ => months,
            };
    }
    let micros = (datetime_micros(end.CoreTime()) - datetime_micros(beg.CoreTime())).abs();
    sign * match unit {
        "WEEK" => micros / 604_800_000_000,
        "DAY" => micros / 86_400_000_000,
        "HOUR" => micros / 3_600_000_000,
        "MINUTE" => micros / 60_000_000,
        "SECOND" => micros / 1_000_000,
        "MICROSECOND" => micros,
        _ => 0,
    }
}

/// 从时间字符串推断小数秒位数（上限 6）。
pub fn GetFsp(value: &str) -> i32 {
    let index = GetFracIndex(value);
    if index < 0 {
        0
    } else {
        let end = GetTimezone(value).0;
        let end = if end < 0 { value.len() } else { end as usize };
        (end.saturating_sub(index as usize + 1)).min(6) as i32
    }
}

/// 返回小数点在时间字符串中的下标；无则 -1。
pub fn GetFracIndex(value: &str) -> i32 {
    let tz = GetTimezone(value).0;
    let end = if tz >= 0 { tz as usize } else { value.len() };
    for (index, ch) in value[..end].char_indices().rev() {
        if ch != '+' && ch != '-' && ch.is_ascii_punctuation() {
            return if ch == '.' { index as i32 } else { -1 };
        }
    }
    -1
}

static TZ_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(Z|([+-])(\d{2})(:?)(\d{0,2}))$").unwrap());

/// 解析末尾时区后缀，返回截断位置与符号/时/分隔/分。
pub fn GetTimezone(value: &str) -> (i32, String, String, String, String) {
    let Some(captures) = TZ_SUFFIX.captures(value) else {
        return (
            -1,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
    };
    let whole = captures.get(1).unwrap();
    let index = whole.start();
    if index < 10 || !value[..index].contains(':') {
        return (
            -1,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
    }
    if whole.as_str().eq_ignore_ascii_case("z") {
        return (
            index as i32,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
    }
    let sign = captures.get(2).map_or("", |m| m.as_str());
    let hour = captures.get(3).map_or("", |m| m.as_str());
    let sep = captures.get(4).map_or("", |m| m.as_str());
    let minute = captures.get(5).map_or("", |m| m.as_str());
    let valid = minute.is_empty() || minute.len() == 2;
    if !valid {
        return (
            -1,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
    }
    (
        index as i32,
        sign.into(),
        hour.into(),
        sep.into(),
        minute.into(),
    )
}

/// 按分隔符拆分日期时间字符串为数字段。
pub fn ParseDateFormat(format: &str) -> Vec<String> {
    let format = format.trim();
    if format.is_empty() || !format.as_bytes()[0].is_ascii_digit() {
        return Vec::new();
    }
    let bytes = format.as_bytes();
    let mut parts = Vec::with_capacity(6);
    let mut start = 0;
    let mut i = 1;
    while i + 1 < bytes.len() {
        if isValidSeparator(bytes[i], parts.len()) {
            let previous = parts.len();
            parts.push(format[start..i].to_owned());
            i += 1;
            start = i;
            while i < bytes.len() && isValidSeparator(bytes[i], previous) {
                i += 1;
                start += 1;
            }
            continue;
        }
        if !bytes[i].is_ascii_digit() {
            return Vec::new();
        }
        i += 1;
    }
    parts.push(format[start..].to_owned());
    parts
}

/// 判断当前字符是否可作为字段分隔符。
fn isValidSeparator(ch: u8, previous_parts: usize) -> bool {
    ch.is_ascii_punctuation()
        || (previous_parts == 2 && matches!(ch, b'T' | b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        || (previous_parts > 4 && !ch.is_ascii_digit())
}

/// 对 NaiveDateTime 按 FSP 四舍五入。
pub fn RoundFrac(t: NaiveDateTime, fsp: i32) -> Result<NaiveDateTime, TimeError> {
    let fsp = CheckFsp(fsp)?;
    let unit = 10_i64.pow((9 - fsp) as u32);
    let nanos = t.nanosecond() as i64;
    Ok(t.with_nanosecond(0).unwrap()
        + ChronoDuration::nanoseconds(((nanos + unit / 2) / unit) * unit))
}

/// 对 NaiveDateTime 按 FSP 截断（不进位）。
pub fn TruncateFrac(t: NaiveDateTime, fsp: i32) -> Result<NaiveDateTime, TimeError> {
    let fsp = CheckFsp(fsp)?;
    let unit = 10_u32.pow((9 - fsp) as u32);
    Ok(t.with_nanosecond(t.nanosecond() / unit * unit).unwrap())
}

/// 解析 YEAR 字符串。
pub fn ParseYear(value: &str) -> Result<i16, TimeError> {
    let year = value
        .trim()
        .parse::<i64>()
        .map_err(|_| TimeError::wrong("year", value))?;
    Ok(AdjustYear(year, true)? as i16)
}

/// 两位年份按 MySQL 规则映射到四位。
fn adjustYear(year: i32) -> i32 {
    if year >= 0 && year <= 69 {
        year + 2000
    } else if year >= 70 && year <= 99 {
        year + 1900
    } else {
        year
    }
}

/// 调整年份；可选把 0 映射为 2000，并检查 YEAR 范围。
pub fn AdjustYear(year: i64, adjust_zero: bool) -> Result<i64, TimeError> {
    let adjusted = if year == 0 && !adjust_zero {
        0
    } else if (0..=69).contains(&year) {
        year + 2000
    } else if (70..=99).contains(&year) {
        year + 1900
    } else {
        year
    };
    if adjusted != 0 && !(MinYear as i64..=MaxYear as i64).contains(&adjusted) {
        return Err(TimeError::wrong("year", year));
    }
    Ok(adjusted)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// MySQL TIME（时长）类型：纳秒级 Duration 与 FSP。
pub struct Duration {
    pub Duration: i64,
    pub Fsp: i32,
}

impl Duration {
    /// 由时分秒微秒与 FSP 构造 Duration。
    pub fn from_parts(hour: i32, minute: i32, second: i32, microsecond: i32, fsp: i32) -> Self {
        Self {
            Duration: ((hour as i64 * 3600 + minute as i64 * 60 + second as i64) * 1_000_000
                + microsecond as i64)
                * 1_000,
            Fsp: fsp.clamp(0, 6),
        }
    }

    /// 取负。
    pub fn Neg(self) -> Duration {
        Duration {
            Duration: -self.Duration,
            Fsp: self.Fsp,
        }
    }

    /// 时长相加，检查 MySQL TIME 溢出。
    pub fn Add(self, other: Duration) -> Result<Duration, TimeError> {
        let value = self
            .Duration
            .checked_add(other.Duration)
            .ok_or_else(|| TimeError::overflow("duration"))?;
        if value.unsigned_abs() > MaxTime as u64 {
            return Err(TimeError::overflow("duration"));
        }
        Ok(Duration {
            Duration: value,
            Fsp: self.Fsp.max(other.Fsp),
        })
    }

    /// 时长相减。
    pub fn Sub(self, other: Duration) -> Result<Duration, TimeError> {
        self.Add(other.Neg())
    }

    /// TIME_FORMAT 风格格式化。
    pub fn DurationFormat(self, layout: &str) -> Result<String, TimeError> {
        let (_, hour, minute, second, micro) = splitDuration(self.Duration);
        let mut out = String::new();
        let mut chars = layout.chars();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                out.push(ch);
                continue;
            }
            let token = chars
                .next()
                .ok_or_else(|| TimeError("trailing % in duration format".into()))?;
            match token {
                '%' => out.push('%'),
                'H' => write!(out, "{:02}", hour).unwrap(),
                'k' => write!(out, "{}", hour).unwrap(),
                'h' | 'I' => write!(out, "{:02}", hour12(hour)).unwrap(),
                'l' => write!(out, "{}", hour12(hour)).unwrap(),
                'i' => write!(out, "{:02}", minute).unwrap(),
                's' | 'S' => write!(out, "{:02}", second).unwrap(),
                'f' => write!(out, "{:06}", micro).unwrap(),
                'p' => out.push_str(if hour % 24 < 12 { "AM" } else { "PM" }),
                'r' => write!(
                    out,
                    "{:02}:{:02}:{:02} {}",
                    hour12(hour),
                    minute,
                    second,
                    if hour % 24 < 12 { "AM" } else { "PM" }
                )
                .unwrap(),
                'T' => write!(out, "{:02}:{:02}:{:02}", hour, minute, second).unwrap(),
                other => out.push(other),
            }
        }
        Ok(out)
    }

    /// 默认文本形式（含符号与 FSP）。
    pub fn String(self) -> String {
        let (sign, hour, minute, second, micro) = splitDuration(self.Duration);
        let mut out = format!(
            "{}{:02}:{:02}:{:02}",
            if sign < 0 { "-" } else { "" },
            hour,
            minute,
            second
        );
        if self.Fsp > 0 {
            let frac = format!("{:06}", micro);
            out.push('.');
            out.push_str(&frac[..self.Fsp as usize]);
        }
        out
    }

    /// 转为 DECIMAL。
    pub fn ToNumber(self) -> Decimal {
        Decimal::from_str(&self.String().replace(':', "")).unwrap_or(Decimal::ZERO)
    }

    /// 结合当前时刻转为 DATE/DATETIME/TIMESTAMP。
    pub fn ConvertToTime<C: TimeContext>(self, ctx: &C, tp: u8) -> Result<Time, TimeError> {
        self.ConvertToTimeWithTimestamp(ctx, tp, Utc::now().with_timezone(&ctx.location()))
    }

    /// 以给定时间戳为基准把时长转为日历时间。
    pub fn ConvertToTimeWithTimestamp<C: TimeContext>(
        self,
        ctx: &C,
        tp: u8,
        now: DateTime<Tz>,
    ) -> Result<Time, TimeError> {
        let date = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        let value = date
            .checked_add_signed(ChronoDuration::nanoseconds(self.Duration))
            .ok_or_else(|| TimeError::overflow(self.String()))?;
        let result = NewTime(
            FromDate(
                value.year(),
                value.month() as i32,
                value.day() as i32,
                value.hour() as i32,
                value.minute() as i32,
                value.second() as i32,
                value.nanosecond() as i32 / 1_000,
            ),
            tp,
            self.Fsp,
        );
        result.Check(ctx)?;
        Ok(result)
    }

    /// 转为 YEAR。
    pub fn ConvertToYear<C: TimeContext>(self, ctx: &C) -> Result<i64, TimeError> {
        self.ConvertToYearFromNow(ctx, Utc::now().with_timezone(&ctx.location()))
    }

    /// 以指定 now 为基准的 YEAR 转换（含 concat 特殊路径）。
    pub fn ConvertToYearFromNow<C: TimeContext>(
        self,
        ctx: &C,
        now: DateTime<Tz>,
    ) -> Result<i64, TimeError> {
        if ctx.flags().cast_time_to_year_through_concat {
            let rounded = self.RoundFrac(DefaultFsp, ctx.location())?;
            let value: i64 = rounded
                .ToNumber()
                .trunc()
                .to_string()
                .parse()
                .map_err(|_| TimeError::wrong("year", rounded.String()))?;
            return AdjustYear(value, false);
        }
        let time = self.ConvertToTimeWithTimestamp(ctx, mysql::TypeDatetime, now)?;
        AdjustYear(time.Year() as i64, false)
    }

    /// 按时长 FSP 四舍五入。
    pub fn RoundFrac(self, fsp: i32, _loc: Tz) -> Result<Duration, TimeError> {
        let fsp = CheckFsp(fsp)?;
        let unit = 10_i64.pow((9 - fsp) as u32);
        let sign = self.Duration.signum();
        let value = ((self.Duration.abs() + unit / 2) / unit) * unit * sign;
        if value.unsigned_abs() > MaxTime as u64 {
            return Err(TimeError::overflow(self.String()));
        }
        Ok(Duration {
            Duration: value,
            Fsp: fsp,
        })
    }

    /// 比较两个 Duration。
    pub fn Compare(self, other: Duration) -> i32 {
        match self.Duration.cmp(&other.Duration) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    /// 与字符串解析出的 Duration 比较。
    pub fn CompareString<C: TimeContext>(self, ctx: &C, value: &str) -> Result<i32, TimeError> {
        let (other, is_null) = ParseDuration(ctx, value, MaxFsp)?;
        if is_null {
            return Err(TimeError::wrong("time", value));
        }
        Ok(self.Compare(other))
    }

    /// 绝对值小时部分。
    pub fn Hour(self) -> i32 {
        splitDuration(self.Duration).1
    }
    /// 绝对值分钟部分。
    pub fn Minute(self) -> i32 {
        splitDuration(self.Duration).2
    }
    /// 绝对值秒部分。
    pub fn Second(self) -> i32 {
        splitDuration(self.Duration).3
    }
    /// 绝对值微秒部分。
    pub fn MicroSecond(self) -> i32 {
        splitDuration(self.Duration).4
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}

/// 便捷构造 Duration。
pub fn NewDuration(hour: i32, minute: i32, second: i32, microsecond: i32, fsp: i32) -> Duration {
    Duration::from_parts(hour, minute, second, microsecond, fsp)
}

/// MySQL TIME 最大正时长。
pub fn MaxMySQLDuration(fsp: i32) -> Duration {
    Duration::from_parts(TimeMaxHour, TimeMaxMinute, TimeMaxSecond, 0, fsp)
}

/// 把纳秒时长拆成符号、时、分、秒、微秒。
pub fn splitDuration(value: i64) -> (i32, i32, i32, i32, i32) {
    let sign = if value < 0 { -1 } else { 1 };
    let micros = value.unsigned_abs() / 1_000;
    let seconds = micros / 1_000_000;
    (
        sign,
        (seconds / 3600) as i32,
        ((seconds / 60) % 60) as i32,
        (seconds % 60) as i32,
        (micros % 1_000_000) as i32,
    )
}

/// 解析 TIME 字符串；可回退尝试 DATETIME。
pub fn ParseDuration<C: TimeContext>(
    ctx: &C,
    value: &str,
    fsp: i32,
) -> Result<(Duration, bool), TimeError> {
    match matchDuration(value, fsp) {
        Ok(result) => Ok(result),
        Err(duration_error) if canFallbackToDateTime(value) => {
            match ParseTime(ctx, value, mysql::TypeDatetime, fsp) {
                Ok(time) => Ok((time.ConvertToDuration()?, false)),
                Err(_) => Err(duration_error),
            }
        }
        Err(error) => Err(error),
    }
}

/// 匹配纯时长形态并解析。
fn matchDuration(value: &str, fsp: i32) -> Result<(Duration, bool), TimeError> {
    let fsp = CheckFsp(fsp)?;
    let mut input = value.trim();
    if input.is_empty() {
        return Err(TimeError::wrong("time", value));
    }
    let negative = input.starts_with('-');
    if input.starts_with(['-', '+']) {
        input = &input[1..];
    }
    let mut day = 0_i64;
    let mut has_day = false;
    if let Some((prefix, rest)) = input.split_once(char::is_whitespace) {
        if prefix.chars().all(|c| c.is_ascii_digit()) {
            day = prefix
                .parse()
                .map_err(|_| TimeError::wrong("time", value))?;
            has_day = true;
            input = rest.trim_start();
        }
    }
    let (main, frac) = input.split_once('.').unwrap_or((input, ""));
    let hms: Vec<&str> = main.split(':').collect();
    let (hour, minute, second) = match hms.as_slice() {
        [h, m, s] => (parse_i64(h)?, parse_i64(m)?, parse_i64(s)?),
        [h, m] => (parse_i64(h)?, parse_i64(m)?, 0),
        [hour] if has_day && hour.chars().all(|c| c.is_ascii_digit()) => (parse_i64(hour)?, 0, 0),
        [compact] if compact.chars().all(|c| c.is_ascii_digit()) => {
            let number = parse_i64(compact)?;
            (number / 10_000, (number / 100) % 100, number % 100)
        }
        _ => return Err(TimeError::wrong("time", value)),
    };
    if minute > 59 || second > 59 {
        return Err(TimeError::wrong("time", value));
    }
    let (micro, carry) = parse_fraction(frac, fsp)?;
    let total_seconds = day * 86_400 + hour * 3_600 + minute * 60 + second + carry;
    if total_seconds > TimeMaxValueSeconds {
        return Err(TimeError::overflow(value));
    }
    let mut nanos = (total_seconds * 1_000_000 + micro) * 1_000;
    if negative {
        nanos = -nanos;
    }
    Ok((
        Duration {
            Duration: nanos,
            Fsp: fsp,
        },
        false,
    ))
}

/// 时长解析失败时是否允许按 DATETIME 再试。
fn canFallbackToDateTime(value: &str) -> bool {
    let digits = value.chars().filter(|c| c.is_ascii_digit()).count();
    value.contains('-') && digits >= 8 || digits >= 12
}

/// 将超出 MySQL TIME 范围的纳秒值截断到边界。
pub fn TruncateOverflowMySQLTime(value: i64) -> (i64, Option<TimeError>) {
    if value > MaxTime {
        (MaxTime, Some(TimeError::overflow(value)))
    } else if value < MinTime {
        (MinTime, Some(TimeError::overflow(value)))
    } else {
        (value, None)
    }
}

/// 按目标类型解析时间字符串。
pub fn ParseTime<C: TimeContext>(
    ctx: &C,
    value: &str,
    tp: u8,
    fsp: i32,
) -> Result<Time, TimeError> {
    parseTime(ctx, value, tp, fsp, false)
}

/// 泛型字符串入口的 ParseTime。
pub fn ParseTimeWithString<C: TimeContext, S: AsRef<str>>(
    ctx: &C,
    value: S,
    tp: u8,
    fsp: i32,
) -> Result<Time, TimeError> {
    parseTime(ctx, value.as_ref(), tp, fsp, false)
}

/// 从浮点数字符串解析时间。
pub fn ParseTimeFromFloatString<C: TimeContext>(
    ctx: &C,
    value: &str,
    tp: u8,
    fsp: i32,
) -> Result<Time, TimeError> {
    // MySQL compatibility: a floating zero remains a zero value of the
    // requested target type instead of being treated as an invalid date.
    if value.starts_with("0.0") {
        return Ok(NewTime(CoreTime(0), tp, DefaultFsp));
    }
    parseTime(ctx, value, tp, fsp, true)
}

/// 内部解析：拆分字段、补全、校验并构造 Time。
fn parseTime<C: TimeContext>(
    ctx: &C,
    value: &str,
    tp: u8,
    fsp: i32,
    is_float: bool,
) -> Result<Time, TimeError> {
    let fsp = CheckFsp(fsp)?;
    let (tz_index, tz_sign, tz_hour, _, tz_minute) = GetTimezone(value);
    let body = if tz_index >= 0 {
        &value[..tz_index as usize]
    } else {
        value
    };
    let body = body.trim();
    if body.chars().all(|c| c.is_ascii_digit()) {
        return parse_digit_datetime_string(ctx, body, tp, fsp);
    }
    let mut normalized = body.replace('T', " ");
    if normalized.contains('/') {
        normalized = normalized.replace('/', "-");
    }
    let frac_index = GetFracIndex(&normalized);
    let (main, frac) = if frac_index >= 0 {
        let index = frac_index as usize;
        (&normalized[..index], &normalized[index + 1..])
    } else {
        (normalized.as_str(), "")
    };
    let mut parts = ParseDateFormat(main);
    let can_absorb_fraction = parts.len() <= 5 && !(parts.len() == 1 && parts[0].len() > 4);
    let frac = if !frac.is_empty() && can_absorb_fraction {
        parts.push(frac.to_owned());
        ""
    } else {
        frac
    };
    if is_float && parts.len() == 1 && main.chars().all(|c| c.is_ascii_digit()) {
        let mut result = parse_datetime_from_num(ctx, parse_i64(main)?)?;
        let inferred_type = result.Type();
        result.SetType(tp);
        result.SetFsp(fsp);
        if inferred_type == mysql::TypeDatetime {
            let (micro, carry) = parse_fraction(frac, fsp)?;
            result.SetCoreTime(FromDate(
                result.Year(),
                result.Month(),
                result.Day(),
                result.Hour(),
                result.Minute(),
                result.Second(),
                micro as i32,
            ));
            if carry != 0 {
                result = result.Add(ctx, Duration::from_parts(0, 0, carry as i32, 0, fsp))?;
            }
        }
        result.Check(ctx)?;
        return Ok(result);
    }
    if parts.len() == 1 && main.chars().all(|c| c.is_ascii_digit()) {
        let mut result = parse_digit_datetime_string(ctx, main, tp, fsp)?;
        if matches!(main.len(), 5 | 6 | 8) && !is_float {
            let compact = frac;
            let hour = compact
                .get(0..compact.len().min(2))
                .unwrap_or("")
                .parse()
                .unwrap_or(0);
            let minute = compact
                .get(2..compact.len().min(4))
                .unwrap_or("")
                .parse()
                .unwrap_or(0);
            let second = compact
                .get(4..compact.len().min(6))
                .unwrap_or("")
                .parse()
                .unwrap_or(0);
            result.SetCoreTime(FromDate(
                result.Year(),
                result.Month(),
                result.Day(),
                hour,
                minute,
                second,
                0,
            ));
        } else {
            let (micro, carry) = parse_fraction(frac, fsp)?;
            result.SetCoreTime(FromDate(
                result.Year(),
                result.Month(),
                result.Day(),
                result.Hour(),
                result.Minute(),
                result.Second(),
                micro as i32,
            ));
            if carry != 0 {
                result = result.Add(ctx, Duration::from_parts(0, 0, carry as i32, 0, fsp))?;
            }
        }
        result.Check(ctx)?;
        return Ok(result);
    }
    if parts.len() < 3 {
        return Err(TimeError::wrong("datetime", value));
    }
    let mut fields = [0_i32; 6];
    for (index, part) in parts.iter().take(6).enumerate() {
        fields[index] = part
            .parse()
            .map_err(|_| TimeError::wrong("datetime", value))?;
    }
    let is_zero = fields.iter().all(|field| *field == 0) && frac.is_empty();
    if parts[0].len() <= 2 && !is_float && !is_zero {
        fields[0] = adjustYear(fields[0]);
    }
    let (micro, carry) = parse_fraction(frac, fsp)?;
    let (core_time, valid_fields) = FromDateChecked(
        fields[0],
        fields[1],
        fields[2],
        fields[3],
        fields[4],
        fields[5],
        micro as i32,
    );
    if !valid_fields {
        return Err(TimeError::wrong("datetime", value));
    }

    // Go keeps zero month/day in CoreTime and lets Time::Check apply the context flags.
    // A calendar conversion is only required for fractional carry or an explicit time zone.
    let mut result = NewTime(core_time, tp, fsp);
    let mut calendar_time = if carry != 0 || tz_index >= 0 {
        Some(
            NaiveDate::from_ymd_opt(fields[0], fields[1] as u32, fields[2] as u32)
                .and_then(|date| {
                    date.and_hms_micro_opt(
                        fields[3] as u32,
                        fields[4] as u32,
                        fields[5] as u32,
                        micro as u32,
                    )
                })
                .ok_or_else(|| TimeError::wrong("datetime", value))?
                + ChronoDuration::seconds(carry),
        )
    } else {
        None
    };
    if let Some(base) = calendar_time {
        result.SetCoreTime(FromDate(
            base.year(),
            base.month() as i32,
            base.day() as i32,
            base.hour() as i32,
            base.minute() as i32,
            base.second() as i32,
            base.nanosecond() as i32 / 1_000,
        ));
    }
    if tz_index >= 0 {
        let utc = if value[tz_index as usize..].eq_ignore_ascii_case("z") {
            chrono::FixedOffset::east_opt(0).unwrap()
        } else {
            let hour: i32 = tz_hour
                .parse()
                .map_err(|_| TimeError::wrong("timezone", value))?;
            let minute: i32 = if tz_minute.is_empty() {
                0
            } else {
                tz_minute
                    .parse()
                    .map_err(|_| TimeError::wrong("timezone", value))?
            };
            if hour > 14 || minute > 59 || (hour == 14 && minute != 0) {
                return Err(TimeError::wrong("timezone", value));
            }
            let seconds = (hour * 3600 + minute * 60) * if tz_sign == "-" { -1 } else { 1 };
            chrono::FixedOffset::east_opt(seconds)
                .ok_or_else(|| TimeError::wrong("timezone", value))?
        };
        let instant = utc
            .from_local_datetime(
                &calendar_time
                    .take()
                    .expect("timezone requires calendar time"),
            )
            .single()
            .ok_or_else(|| TimeError::wrong("datetime", value))?;
        result.SetCoreTime(FromGoTime(instant.with_timezone(&ctx.location())));
    }
    result.Check(ctx)?;
    Ok(result)
}

/// 解析紧凑数字串形式的日期时间（如 20000102）。
fn parse_digit_datetime_string<C: TimeContext>(
    ctx: &C,
    digits: &str,
    tp: u8,
    fsp: i32,
) -> Result<Time, TimeError> {
    let take = |start: usize, width: usize| -> Result<i32, TimeError> {
        if start >= digits.len() {
            return Ok(0);
        }
        let end = (start + width).min(digits.len());
        parse_i32(&digits[start..end])
    };
    let (mut year, month, day, hour, minute, second, short_year) = match digits.len() {
        14 => (
            take(0, 4)?,
            take(4, 2)?,
            take(6, 2)?,
            take(8, 2)?,
            take(10, 2)?,
            take(12, 2)?,
            false,
        ),
        12 => (
            take(0, 2)?,
            take(2, 2)?,
            take(4, 2)?,
            take(6, 2)?,
            take(8, 2)?,
            take(10, 2)?,
            true,
        ),
        11 => (
            take(0, 2)?,
            take(2, 2)?,
            take(4, 2)?,
            take(6, 2)?,
            take(8, 2)?,
            take(10, 1)?,
            true,
        ),
        10 => (
            take(0, 2)?,
            take(2, 2)?,
            take(4, 2)?,
            take(6, 2)?,
            take(8, 2)?,
            0,
            true,
        ),
        9 => (
            take(0, 2)?,
            take(2, 2)?,
            take(4, 2)?,
            take(6, 2)?,
            take(8, 1)?,
            0,
            true,
        ),
        8 => (take(0, 4)?, take(4, 2)?, take(6, 2)?, 0, 0, 0, false),
        7 => (
            take(0, 2)?,
            take(2, 2)?,
            take(4, 2)?,
            take(6, 1)?,
            0,
            0,
            true,
        ),
        5 | 6 => (take(0, 2)?, take(2, 2)?, take(4, 2)?, 0, 0, 0, true),
        _ => return Err(TimeError::wrong("time", digits)),
    };
    if short_year {
        year = adjustYear(year);
    }
    let (core, valid) = FromDateChecked(year, month, day, hour, minute, second, 0);
    if !valid {
        return Err(TimeError::wrong("datetime", digits));
    }
    let result = NewTime(core, tp, fsp);
    result.Check(ctx)?;
    Ok(result)
}

/// 解析为 DATETIME。
pub fn ParseDatetime<C: TimeContext>(ctx: &C, value: &str) -> Result<Time, TimeError> {
    ParseTime(ctx, value, mysql::TypeDatetime, GetFsp(value))
}
/// 解析为 TIMESTAMP。
pub fn ParseTimestamp<C: TimeContext>(ctx: &C, value: &str) -> Result<Time, TimeError> {
    ParseTime(ctx, value, mysql::TypeTimestamp, GetFsp(value))
}
/// 解析为 DATE。
pub fn ParseDate<C: TimeContext>(ctx: &C, value: &str) -> Result<Time, TimeError> {
    ParseTime(ctx, value, mysql::TypeDate, 0)
}

/// 从 YEAR 值构造 DATE（x-00-00）。
pub fn ParseTimeFromYear(year: i64) -> Result<Time, TimeError> {
    let year = AdjustYear(year, true)? as i32;
    Ok(NewTime(
        FromDate(year, 1, 1, 0, 0, 0, 0),
        mysql::TypeDate,
        0,
    ))
}

/// 从整数按目标类型解析时间。
pub fn ParseTimeFromNum<C: TimeContext>(
    ctx: &C,
    value: i64,
    tp: u8,
    fsp: i32,
) -> Result<Time, TimeError> {
    if value == 0 {
        return Ok(NewTime(CoreTime(0), tp, DefaultFsp));
    }
    let fsp = CheckFsp(fsp)?;
    let mut result = parse_datetime_from_num(ctx, value)?;
    result.SetType(tp);
    result.SetFsp(fsp);
    result.Check(ctx)?;
    Ok(result)
}

/// 把整数按位数拆成日期时间分量。
fn parse_datetime_from_num<C: TimeContext>(ctx: &C, value: i64) -> Result<Time, TimeError> {
    if value == 0 {
        return Ok(NewTime(CoreTime(0), mysql::TypeDate, DefaultFsp));
    }
    if value < 101 {
        return Err(TimeError::wrong("time", value));
    }

    let mut num = value;
    let mut tp = mysql::TypeDate;
    if num >= 10_000_101_000_000 {
        tp = mysql::TypeDatetime;
    } else if num <= 691_231 {
        num = (num + 20_000_000) * 1_000_000;
    } else if num < 700_101 {
        return Err(TimeError::wrong("time", value));
    } else if num <= 991_231 {
        num = (num + 19_000_000) * 1_000_000;
    } else if num <= 99_991_231 {
        num *= 1_000_000;
    } else {
        if num < 101_000_000 {
            return Err(TimeError::wrong("time", value));
        }
        tp = mysql::TypeDatetime;
        if num <= 6_912_312_359_59 {
            num += 20_000_000_000_000;
        } else if num < 7_001_010_000_00 {
            return Err(TimeError::wrong("time", value));
        } else if num <= 9_912_312_359_59 {
            num += 19_000_000_000_000;
        }
    }

    let year = num / 10_000_000_000;
    let month = num / 100_000_000 % 100;
    let day = num / 1_000_000 % 100;
    let hour = num / 10_000 % 100;
    let minute = num / 100 % 100;
    let second = num % 100;
    let (core, valid) = FromDateChecked(
        year as i32,
        month as i32,
        day as i32,
        hour as i32,
        minute as i32,
        second as i32,
        0,
    );
    if !valid {
        return Err(TimeError::wrong("time", value));
    }
    let result = NewTime(core, tp, DefaultFsp);
    result.Check(ctx)?;
    Ok(result)
}

/// 整数解析为 DATETIME。
pub fn ParseDatetimeFromNum<C: TimeContext>(ctx: &C, value: i64) -> Result<Time, TimeError> {
    ParseTimeFromNum(ctx, value, mysql::TypeDatetime, 0)
}
/// 整数解析为 TIMESTAMP。
pub fn ParseTimestampFromNum<C: TimeContext>(ctx: &C, value: i64) -> Result<Time, TimeError> {
    ParseTimeFromNum(ctx, value, mysql::TypeTimestamp, 0)
}
/// 整数解析为 DATE。
pub fn ParseDateFromNum<C: TimeContext>(ctx: &C, value: i64) -> Result<Time, TimeError> {
    ParseTimeFromNum(ctx, value, mysql::TypeDate, 0)
}
/// 从 i64 推断类型并解析。
pub fn ParseTimeFromInt64<C: TimeContext>(ctx: &C, value: i64) -> Result<Time, TimeError> {
    parse_datetime_from_num(ctx, value)
}
/// 从 f64 解析时间（小数部分为分数秒）。
pub fn ParseTimeFromFloat64<C: TimeContext>(ctx: &C, value: f64) -> Result<Time, TimeError> {
    let int_part = value.trunc() as i64;
    let mut result = parse_datetime_from_num(ctx, int_part)?;
    if result.Type() == mysql::TypeDatetime {
        let micro = ((value - int_part as f64) * 1_000_000.0).round() as i32;
        result.SetCoreTime(FromDate(
            result.Year(),
            result.Month(),
            result.Day(),
            result.Hour(),
            result.Minute(),
            result.Second(),
            micro,
        ));
    }
    Ok(result)
}
/// 从 Decimal 解析时间。
pub fn ParseTimeFromDecimal<C: TimeContext>(ctx: &C, value: &Decimal) -> Result<Time, TimeError> {
    let int_part: i64 = value
        .trunc()
        .to_string()
        .parse()
        .map_err(|_| TimeError::wrong("time", value))?;
    let mut result = parse_datetime_from_num(ctx, int_part)?;
    result.SetFsp(value.scale().min(MaxFsp as u32) as i32);
    if result.Type() == mysql::TypeDatetime && result.Fsp() != 0 {
        let micros = ((*value - Decimal::from(int_part)) * Decimal::from(1_000_000_i64)).trunc();
        let micro: i32 = micros
            .to_string()
            .parse()
            .map_err(|_| TimeError::wrong("time", value))?;
        result.SetCoreTime(FromDate(
            result.Year(),
            result.Month(),
            result.Day(),
            result.Hour(),
            result.Minute(),
            result.Second(),
            micro,
        ));
    }
    Ok(result)
}

/// 从日序号（MySQL FROM_DAYS）构造 DATE。
pub fn TimeFromDays(days: i64) -> Time {
    let (year, month, day) = get_date_from_daynr(days.max(0) as u32);
    NewTime(
        FromDate(year as i32, month as i32, day as i32, 0, 0, 0, 0),
        mysql::TypeDate,
        0,
    )
}

/// EXTRACT 日期时间单位为整数。
pub fn ExtractDatetimeNum(time: &Time, unit: &str) -> Result<i64, TimeError> {
    let unit = unit.to_ascii_uppercase();
    let value = match unit.as_str() {
        "DAY" => time.Day() as i64,
        "WEEK" => week_number(time.CoreTime(), false, false) as i64,
        "MONTH" => time.Month() as i64,
        "QUARTER" => ((time.Month() + 2) / 3) as i64,
        "YEAR" => time.Year() as i64,
        "YEAR_MONTH" => time.Year() as i64 * 100 + time.Month() as i64,
        "DAY_HOUR" => time.Day() as i64 * 100 + time.Hour() as i64,
        "DAY_MINUTE" => {
            time.Day() as i64 * 10_000 + time.Hour() as i64 * 100 + time.Minute() as i64
        }
        "DAY_SECOND" => {
            time.Day() as i64 * 1_000_000
                + time.Hour() as i64 * 10_000
                + time.Minute() as i64 * 100
                + time.Second() as i64
        }
        "DAY_MICROSECOND" => {
            (time.Day() as i64 * 1_000_000
                + time.Hour() as i64 * 10_000
                + time.Minute() as i64 * 100
                + time.Second() as i64)
                * 1_000_000
                + time.Microsecond() as i64
        }
        "HOUR" => time.Hour() as i64,
        "HOUR_MINUTE" => time.Hour() as i64 * 100 + time.Minute() as i64,
        "HOUR_SECOND" => {
            time.Hour() as i64 * 10_000 + time.Minute() as i64 * 100 + time.Second() as i64
        }
        "HOUR_MICROSECOND" => {
            (time.Hour() as i64 * 10_000 + time.Minute() as i64 * 100 + time.Second() as i64)
                * 1_000_000
                + time.Microsecond() as i64
        }
        "MINUTE" => time.Minute() as i64,
        "MINUTE_SECOND" => time.Minute() as i64 * 100 + time.Second() as i64,
        "MINUTE_MICROSECOND" => {
            (time.Minute() as i64 * 100 + time.Second() as i64) * 1_000_000
                + time.Microsecond() as i64
        }
        "SECOND" => time.Second() as i64,
        "SECOND_MICROSECOND" | "MICROSECOND" => {
            time.Second() as i64 * 1_000_000 + time.Microsecond() as i64
        }
        _ => return Err(TimeError(format!("invalid unit {unit}"))),
    };
    Ok(value)
}

/// EXTRACT 时长单位为整数。
pub fn ExtractDurationNum(duration: &Duration, unit: &str) -> Result<i64, TimeError> {
    let sign = if duration.Duration < 0 { -1_i64 } else { 1 };
    let (_, hour, minute, second, micro) = splitDuration(duration.Duration);
    let unit = unit.to_ascii_uppercase();
    let value = match unit.as_str() {
        "MICROSECOND" => micro as i64,
        "HOUR" => hour as i64,
        "HOUR_MINUTE" => hour as i64 * 100 + minute as i64,
        "HOUR_SECOND" => hour as i64 * 10_000 + minute as i64 * 100 + second as i64,
        "HOUR_MICROSECOND" => {
            (hour as i64 * 10_000 + minute as i64 * 100 + second as i64) * 1_000_000 + micro as i64
        }
        "MINUTE" => minute as i64,
        "MINUTE_SECOND" => minute as i64 * 100 + second as i64,
        "MINUTE_MICROSECOND" => (minute as i64 * 100 + second as i64) * 1_000_000 + micro as i64,
        "SECOND" => second as i64,
        "SECOND_MICROSECOND" => second as i64 * 1_000_000 + micro as i64,
        "DAY_HOUR" => hour as i64,
        "DAY_MINUTE" => hour as i64 * 100 + minute as i64,
        "DAY_SECOND" => hour as i64 * 10_000 + minute as i64 * 100 + second as i64,
        "DAY_MICROSECOND" => {
            (hour as i64 * 10_000 + minute as i64 * 100 + second as i64) * 1_000_000 + micro as i64
        }
        _ => return Err(TimeError(format!("invalid unit {unit}"))),
    };
    Ok(sign * value)
}

/// 解析 INTERVAL 值字符串，返回年月/日/纳秒等分量。
pub fn ParseDurationValue(
    unit: &str,
    format: &str,
) -> Result<(i64, i64, i64, i64, i32), TimeError> {
    let unit = unit.to_ascii_uppercase();
    match unit.as_str() {
        "MICROSECOND" | "SECOND" | "MINUTE" | "HOUR" | "DAY" | "WEEK" | "MONTH" | "QUARTER"
        | "YEAR" => parse_single_duration_value(&unit, format, false),
        "SECOND_MICROSECOND" => parse_composite_duration_value(format, MicrosecondIndex, 2),
        "MINUTE_MICROSECOND" => parse_composite_duration_value(format, MicrosecondIndex, 3),
        "MINUTE_SECOND" => parse_composite_duration_value(format, SecondIndex, 2),
        "HOUR_MICROSECOND" => parse_composite_duration_value(format, MicrosecondIndex, 4),
        "HOUR_SECOND" => parse_composite_duration_value(format, SecondIndex, 3),
        "HOUR_MINUTE" => parse_composite_duration_value(format, MinuteIndex, 2),
        "DAY_MICROSECOND" => parse_composite_duration_value(format, MicrosecondIndex, 5),
        "DAY_SECOND" => parse_composite_duration_value(format, SecondIndex, 4),
        "DAY_MINUTE" => parse_composite_duration_value(format, MinuteIndex, 3),
        "DAY_HOUR" => parse_composite_duration_value(format, HourIndex, 2),
        "YEAR_MONTH" => parse_composite_duration_value(format, MonthIndex, 2),
        _ => return Err(TimeError(format!("invalid unit {unit}"))),
    }
}

/// 把区间字符串提取为 Duration。
pub fn ExtractDurationValue(unit: &str, format: &str) -> Result<Duration, TimeError> {
    let unit = unit.to_ascii_uppercase();
    let single = matches!(
        unit.as_str(),
        "MICROSECOND" | "SECOND" | "MINUTE" | "HOUR" | "DAY" | "WEEK" | "MONTH"
    );
    if matches!(unit.as_str(), "QUARTER" | "YEAR" | "YEAR_MONTH") {
        return Err(TimeError::wrong("time", format));
    }
    let (year, month, day, nanos, fsp) = if single {
        parse_single_duration_value(&unit, format, true)?
    } else {
        ParseDurationValue(&unit, format)?
    };
    if year != 0 || (!single && month != 0) || day.abs() > (TimeMaxHour / 24) as i64 {
        return Err(TimeError::overflow("time"));
    }
    let value = nanos
        .checked_add((month * 30 + day) * GoDurationDay)
        .ok_or_else(|| TimeError::overflow(format))?;
    if value.abs() > MaxTime {
        return Err(TimeError::overflow("time"));
    }
    Ok(Duration {
        Duration: value,
        Fsp: fsp,
    })
}

/// 解析单一单位区间值。
fn parse_single_duration_value(
    unit: &str,
    format: &str,
    strict: bool,
) -> Result<(i64, i64, i64, i64, i32), TimeError> {
    let dot = format.find('.').unwrap_or(format.len());
    let integer = format[..dot]
        .parse::<i64>()
        .map_err(|_| TimeError::wrong("datetime", format))?;
    let sign = if format.starts_with('-') { -1 } else { 1 };
    let fraction = if dot < format.len().saturating_sub(1) {
        format[dot + 1..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>()
    } else {
        String::new()
    };
    let fraction_len = fraction.len() as i32;
    let mut micros = if fraction.is_empty() {
        0
    } else {
        let first = &fraction[..fraction.len().min(6)];
        first.parse::<i64>().unwrap() * 10_i64.pow((6 - first.len()) as u32)
    };
    let mut rounded = integer;
    if micros >= 500_000 {
        rounded += sign;
    }
    micros *= sign;
    let (year, month, mut day, nanos, fsp) = match unit {
        "MICROSECOND" => {
            if strict && rounded.abs() > TimeMaxValueSeconds * 1_000 {
                return Err(TimeError::overflow("time"));
            }
            let day = rounded / 86_400_000_000;
            let value = rounded % 86_400_000_000;
            (0, 0, day, value * 1_000, MaxFsp)
        }
        "SECOND" => {
            if strict && integer.abs() > TimeMaxValueSeconds {
                return Err(TimeError::overflow("time"));
            }
            let day = integer / 86_400;
            let value = integer % 86_400;
            (
                0,
                0,
                day,
                (value * 1_000_000 + micros) * 1_000,
                fraction_len,
            )
        }
        "MINUTE" => (0, 0, rounded / 1_440, rounded % 1_440 * 60_000_000_000, 0),
        "HOUR" => (0, 0, rounded / 24, rounded % 24 * 3_600_000_000_000, 0),
        "DAY" => (0, 0, rounded, 0, 0),
        "WEEK" => (0, 0, rounded * 7, 0, 0),
        "MONTH" => (0, rounded, 0, 0, 0),
        "QUARTER" => (0, rounded * 3, 0, 0, 0),
        "YEAR" => (rounded, 0, 0, 0, 0),
        _ => return Err(TimeError(format!("invalid unit {unit}"))),
    };
    if strict {
        let exceeds = match unit {
            "MINUTE" => rounded.abs() > (TimeMaxHour * 60 + TimeMaxMinute) as i64,
            "HOUR" => rounded.abs() > TimeMaxHour as i64,
            "DAY" => rounded.abs() > (TimeMaxHour / 24) as i64,
            "WEEK" => (rounded * 7).abs() > (TimeMaxHour / 24) as i64,
            "MONTH" => rounded.abs() > 1,
            _ => false,
        };
        if exceeds {
            return Err(TimeError::overflow("time"));
        }
    }
    if !fraction.is_empty() && unit != "SECOND" {
        return Err(TimeError::wrong("datetime", format));
    }
    if nanos.abs() >= GoDurationDay {
        day += nanos / GoDurationDay;
    }
    Ok((year, month, day, nanos % GoDurationDay, fsp))
}

/// 解析复合单位（如 DAY_SECOND）区间值。
fn parse_composite_duration_value(
    format: &str,
    mut index: usize,
    count: usize,
) -> Result<(i64, i64, i64, i64, i32), TimeError> {
    let has_microsecond = index == MicrosecondIndex;
    let input = format.trim();
    let negative = input.starts_with('-');
    let matches: Vec<&str> = Regex::new(r"\d+")
        .unwrap()
        .find_iter(input)
        .map(|m| m.as_str())
        .collect();
    if matches.len() > count {
        return Err(TimeError::wrong("datetime", format));
    }
    let mut fields = [0_i64; TimeValueCnt];
    for value in matches.iter().rev() {
        let mut parsed = value
            .parse::<i64>()
            .map_err(|_| TimeError::wrong("datetime", format))?;
        if negative {
            parsed = -parsed;
        }
        fields[index] = parsed;
        index = index.saturating_sub(1);
    }
    let micro_text = has_microsecond.then(|| matches.last()).flatten().copied();
    if let Some(value) = micro_text {
        if value.len() < 6 {
            fields[MicrosecondIndex] *= 10_i64.pow((6 - value.len()) as u32);
        }
    }
    let seconds = fields[HourIndex] * 3_600 + fields[MinuteIndex] * 60 + fields[SecondIndex];
    let days = fields[DayIndex] + seconds / 86_400;
    let nanos = (seconds % 86_400 * 1_000_000 + fields[MicrosecondIndex]) * 1_000;
    let fsp = if has_microsecond { MaxFsp } else { MinFsp };
    Ok((fields[YearIndex], fields[MonthIndex], days, nanos, fsp))
}

/// 是否为时钟相关 INTERVAL 单位。
pub fn IsClockUnit(unit: &str) -> bool {
    matches!(
        unit.to_ascii_uppercase().as_str(),
        "HOUR"
            | "MINUTE"
            | "SECOND"
            | "MICROSECOND"
            | "DAY_HOUR"
            | "DAY_MINUTE"
            | "DAY_SECOND"
            | "DAY_MICROSECOND"
            | "HOUR_MINUTE"
            | "HOUR_SECOND"
            | "HOUR_MICROSECOND"
            | "MINUTE_SECOND"
            | "MINUTE_MICROSECOND"
            | "SECOND_MICROSECOND"
    )
}
/// 是否为日期相关 INTERVAL 单位。
pub fn IsDateUnit(unit: &str) -> bool {
    matches!(
        unit.to_ascii_uppercase().as_str(),
        "YEAR"
            | "QUARTER"
            | "MONTH"
            | "WEEK"
            | "DAY"
            | "YEAR_MONTH"
            | "DAY_HOUR"
            | "DAY_MINUTE"
            | "DAY_SECOND"
            | "DAY_MICROSECOND"
    )
}
/// 是否涉及微秒的 INTERVAL 单位。
pub fn IsMicrosecondUnit(unit: &str) -> bool {
    matches!(
        unit.to_ascii_uppercase().as_str(),
        "MICROSECOND"
            | "SECOND_MICROSECOND"
            | "MINUTE_MICROSECOND"
            | "HOUR_MICROSECOND"
            | "DAY_MICROSECOND"
    )
}
/// 字符串是否更像日期字面量而非纯时间。
pub fn IsDateFormat(format: &str) -> bool {
    let format = format.trim();
    let parts = ParseDateFormat(format);
    match parts.len() {
        1 => matches!(format.len(), 5 | 6 | 8),
        3 => true,
        _ => false,
    }
}

/// 从 DATE_FORMAT 风格串判断是否含日期/时间说明符。
pub fn GetFormatType(format: &str) -> (bool, bool) {
    let mut duration = false;
    let mut date = false;
    let bytes = format.trim().as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'%' {
            match bytes[index + 1] {
                b'h' | b'H' | b'i' | b'I' | b's' | b'S' | b'k' | b'l' | b'f' | b'r' | b'T' => {
                    duration = true
                }
                b'y' | b'Y' | b'm' | b'M' | b'c' | b'b' | b'D' | b'd' | b'e' => date = true,
                _ => {}
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    (duration, date)
}

/// 定宽补零整数格式化。
pub fn FormatIntWidthN(number: i32, width: usize) -> String {
    format!("{number:0width$}")
}
/// 英文序数日后缀（1st/2nd/3rd/...）。
pub fn abbrDayOfMonth(day: i32) -> String {
    let suffix = if (11..=13).contains(&(day % 100)) {
        "th"
    } else {
        match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{day}{suffix}")
}
/// 从日期/时间串读取小数秒位数。
pub fn DateFSP(date: &str) -> i32 {
    date.rsplit_once('.').map_or(0, |(_, f)| f.len() as i32)
}

/// 判断时间是否超出类型合法范围。
pub fn DateTimeIsOverflow<C: TimeContext>(ctx: &C, date: Time) -> Result<bool, TimeError> {
    let value = date.AdjustedGoTime(ctx.location())?;
    let (min, max) = if date.Type() == mysql::TypeTimestamp {
        (
            MinTimestamp().AdjustedGoTime(ctx.location())?,
            MaxTimestamp().AdjustedGoTime(ctx.location())?,
        )
    } else {
        (
            MinDatetime().AdjustedGoTime(ctx.location())?,
            MaxDatetime().AdjustedGoTime(ctx.location())?,
        )
    };
    Ok(value < min || value > max)
}

/// STR_TO_DATE 核心：按 format 说明符消费输入。
fn parse_str_to_date(date: &str, format: &str) -> Result<(CoreTime, bool), TimeError> {
    let mut input = date.trim_start();
    let mut fields = [0_i32; 7];
    let mut am_pm = None;
    let mut twelve_hour = false;
    let mut twenty_four_hour = false;
    let mut format_chars = format.trim_start().chars().peekable();
    while let Some(ch) = format_chars.next() {
        input = input.trim_start();
        if input.is_empty() {
            break;
        }
        if ch != '%' {
            if ch.is_whitespace() {
                continue;
            } else if input.starts_with(ch) {
                input = &input[ch.len_utf8()..];
            } else {
                return Err(TimeError::wrong("datetime", date));
            }
            continue;
        }
        let token = format_chars
            .next()
            .ok_or_else(|| TimeError("trailing %".into()))?;
        match token {
            'Y' => {
                let digits = take_number(&mut input, 4, &mut fields[0])?;
                if digits <= 2 {
                    fields[0] = adjustYear(fields[0]);
                }
            }
            'y' => {
                take_number(&mut input, 2, &mut fields[0])?;
                fields[0] = adjustYear(fields[0]);
            }
            'm' | 'c' => {
                take_number(&mut input, 2, &mut fields[1])?;
                if fields[1] > 12 {
                    return Err(TimeError::wrong("datetime", date));
                }
            }
            'd' | 'e' => {
                take_number(&mut input, 2, &mut fields[2])?;
                if fields[2] > 31 {
                    return Err(TimeError::wrong("datetime", date));
                }
            }
            'H' | 'k' => {
                take_number(&mut input, 2, &mut fields[3])?;
                if fields[3] > 23 {
                    return Err(TimeError::wrong("datetime", date));
                }
                twenty_four_hour = true;
            }
            'h' | 'I' | 'l' => {
                take_number(&mut input, 2, &mut fields[3])?;
                if !(1..=12).contains(&fields[3]) {
                    return Err(TimeError::wrong("datetime", date));
                }
                twelve_hour = true;
            }
            'i' => {
                take_number(&mut input, 2, &mut fields[4])?;
                if fields[4] > 59 {
                    return Err(TimeError::wrong("datetime", date));
                }
            }
            's' | 'S' => {
                take_number(&mut input, 2, &mut fields[5])?;
                if fields[5] > 59 {
                    return Err(TimeError::wrong("datetime", date));
                }
            }
            'f' => {
                let original = input;
                take_number(&mut input, 6, &mut fields[6])?;
                let consumed = original.len() - input.len();
                fields[6] *= 10_i32.pow((6 - consumed) as u32);
            }
            'r' => {
                let (hour, minute, second) = take_clock(&mut input, true, date)?;
                fields[3] = hour;
                fields[4] = minute;
                fields[5] = second;
            }
            'T' => {
                let (hour, minute, second) = take_clock(&mut input, false, date)?;
                fields[3] = hour;
                fields[4] = minute;
                fields[5] = second;
            }
            'p' => {
                if input.len() < 2 {
                    return Err(TimeError::wrong("datetime", date));
                }
                if input[..2].eq_ignore_ascii_case("am") {
                    am_pm = Some(false);
                } else if input[..2].eq_ignore_ascii_case("pm") {
                    am_pm = Some(true);
                } else {
                    return Err(TimeError::wrong("datetime", date));
                }
                input = &input[2..];
            }
            'b' => {
                let lower = input.to_ascii_lowercase();
                let index = MONTH_ABBREV
                    .iter()
                    .position(|m| lower.starts_with(&m.to_ascii_lowercase()))
                    .ok_or_else(|| TimeError::wrong("month", input))?;
                fields[1] = index as i32 + 1;
                input = &input[3..];
            }
            'M' => {
                let index = MonthNames
                    .iter()
                    .position(|m| {
                        input.len() >= m.len() && input[..m.len()].eq_ignore_ascii_case(m)
                    })
                    .ok_or_else(|| TimeError::wrong("month", input))?;
                fields[1] = index as i32 + 1;
                input = &input[MonthNames[index].len()..];
            }
            '#' => input = input.trim_start_matches(|c: char| c.is_numeric()),
            '.' => input = input.trim_start_matches(|c: char| c.is_ascii_punctuation()),
            '@' => input = input.trim_start_matches(|c: char| c.is_alphabetic()),
            '%' => {
                if !input.starts_with('%') {
                    return Err(TimeError::wrong("datetime", date));
                }
                input = &input[1..];
            }
            other => {
                if !input.starts_with(other) {
                    return Err(TimeError::wrong("datetime", date));
                }
                input = &input[other.len_utf8()..];
            }
        }
    }
    if am_pm.is_some() && twenty_four_hour {
        return Err(TimeError::wrong("datetime", date));
    }
    if let Some(pm) = am_pm {
        if !twelve_hour || fields[3] == 0 {
            return Err(TimeError::wrong("datetime", date));
        }
        if fields[3] == 12 {
            fields[3] = 0;
        }
        if pm {
            fields[3] += 12;
        }
    } else if twelve_hour && fields[3] == 12 {
        fields[3] = 0;
    }
    Ok((
        FromDate(
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6],
        ),
        !input.trim().is_empty(),
    ))
}

/// 从输入前缀取至多 max 位数字。
fn take_number(input: &mut &str, max: usize, output: &mut i32) -> Result<usize, TimeError> {
    let count = input
        .chars()
        .take(max)
        .take_while(|c| c.is_ascii_digit())
        .count();
    if count == 0 {
        return Err(TimeError::wrong("datetime", *input));
    }
    *output = input[..count]
        .parse()
        .map_err(|_| TimeError::wrong("datetime", *input))?;
    *input = &input[count..];
    Ok(count)
}

/// 解析时钟片段到时分秒。
fn take_clock(
    input: &mut &str,
    twelve_hour: bool,
    original: &str,
) -> Result<(i32, i32, i32), TimeError> {
    *input = input.trim_start();
    let mut hour = 0;
    take_number(input, 2, &mut hour)?;
    if (twelve_hour && !(1..=12).contains(&hour)) || (!twelve_hour && hour > 23) {
        return Err(TimeError::wrong("datetime", original));
    }
    if twelve_hour && hour == 12 {
        hour = 0;
    }

    let mut minute = 0;
    let mut second = 0;
    *input = input.trim_start();
    if input.is_empty() {
        return Ok((hour, minute, second));
    }
    if !input.starts_with(':') {
        return Err(TimeError::wrong("datetime", original));
    }
    *input = input[1..].trim_start();
    if input.is_empty() {
        return Ok((hour, minute, second));
    }
    take_number(input, 2, &mut minute)?;
    if minute > 59 {
        return Err(TimeError::wrong("datetime", original));
    }

    *input = input.trim_start();
    if input.is_empty() {
        return Ok((hour, minute, second));
    }
    if !input.starts_with(':') {
        return Err(TimeError::wrong("datetime", original));
    }
    *input = input[1..].trim_start();
    if input.is_empty() {
        return Ok((hour, minute, second));
    }
    take_number(input, 2, &mut second)?;
    if second > 59 {
        return Err(TimeError::wrong("datetime", original));
    }

    if twelve_hour {
        *input = input.trim_start();
        if input.is_empty() {
            return Ok((hour, minute, second));
        }
        if input.len() < 2 {
            return Err(TimeError::wrong("datetime", original));
        }
        if input[..2].eq_ignore_ascii_case("am") {
            *input = &input[2..];
        } else if input[..2].eq_ignore_ascii_case("pm") {
            hour += 12;
            *input = &input[2..];
        } else {
            return Err(TimeError::wrong("datetime", original));
        }
    }
    Ok((hour, minute, second))
}

/// 解析小数秒，返回（截断后的微秒贡献, 进位）。
fn parse_fraction(value: &str, fsp: i32) -> Result<(i64, i64), TimeError> {
    if !value.chars().all(|c| c.is_ascii_digit()) {
        return Err(TimeError::wrong("fraction", value));
    }
    if value.is_empty() {
        return Ok((0, 0));
    }
    let first_six = &value[..value.len().min(6)];
    let mut micro: i64 = first_six
        .parse()
        .map_err(|_| TimeError::wrong("fraction", value))?;
    micro *= 10_i64.pow((6 - first_six.len()) as u32);
    if value.len() > 6 && value.as_bytes()[6] >= b'5' {
        micro += 1;
    }
    let unit = 10_i64.pow((6 - fsp) as u32);
    micro = ((micro + unit / 2) / unit) * unit;
    if micro >= 1_000_000 {
        Ok((0, 1))
    } else {
        Ok((micro, 0))
    }
}

/// 解析 i64。
fn parse_i64(value: &str) -> Result<i64, TimeError> {
    value.parse().map_err(|_| TimeError::wrong("number", value))
}
/// 解析 i32。
fn parse_i32(value: &str) -> Result<i32, TimeError> {
    value.parse().map_err(|_| TimeError::wrong("number", value))
}

/// MySQL 日序号（day number）计算。
fn calc_daynr(mut year: i32, month: i32, day: i32) -> i64 {
    if year == 0 && month == 0 {
        return 0;
    }
    let mut sum = 365_i64 * year as i64 + 31 * (month - 1) as i64 + day as i64;
    if month <= 2 {
        year -= 1;
    } else {
        sum -= ((month * 4 + 23) / 10) as i64;
    }
    sum + (year / 4) as i64 - (((year / 100 + 1) * 3) / 4) as i64
}

/// 日历时间转为自纪元起的微秒近似值（用于差值）。
fn datetime_micros(core: CoreTime) -> i64 {
    (calc_daynr(core.Year(), core.Month(), core.Day()) * 86_400
        + core.Hour() as i64 * 3_600
        + core.Minute() as i64 * 60
        + core.Second() as i64)
        * 1_000_000
        + core.Microsecond() as i64
}

/// 日序号转回年月日。
fn get_date_from_daynr(daynr: u32) -> (u32, u32, u32) {
    if daynr <= 365 || daynr >= 3_652_500 {
        return (0, 0, 0);
    }
    let mut year = daynr * 100 / 36_525;
    let temp = (((year - 1) / 100 + 1) * 3) / 4;
    let mut day_of_year = daynr - year * 365 - (year - 1) / 4 + temp;
    let mut days_in_year = if is_leap(year as i32) { 366 } else { 365 };
    while day_of_year > days_in_year {
        day_of_year -= days_in_year;
        year += 1;
        days_in_year = if is_leap(year as i32) { 366 } else { 365 };
    }
    let month_days = [
        31,
        if is_leap(year as i32) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1;
    for days in month_days {
        if day_of_year <= days {
            break;
        }
        day_of_year -= days;
        month += 1;
    }
    (year, month, day_of_year)
}

/// 是否闰年。
fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
/// 24 小时制转 12 小时制显示。
fn hour12(hour: i32) -> i32 {
    let value = hour % 12;
    if value == 0 { 12 } else { value }
}
/// 一年中的第几天。
fn year_day(core: CoreTime) -> i32 {
    (calc_daynr(core.Year(), core.Month(), core.Day()) - calc_daynr(core.Year(), 1, 1) + 1) as i32
}
/// 星期下标。
fn weekday_index(core: CoreTime) -> usize {
    normalized_naive_date(core.Year(), core.Month(), core.Day())
        .map(|date| match date.weekday() {
            Weekday::Mon => 0,
            Weekday::Tue => 1,
            Weekday::Wed => 2,
            Weekday::Thu => 3,
            Weekday::Fri => 4,
            Weekday::Sat => 5,
            Weekday::Sun => 6,
        })
        .unwrap_or(0)
}

/// 构造 NaiveDate；非法日期返回 None。
fn normalized_naive_date(year: i32, month: i32, day: i32) -> Option<NaiveDate> {
    let month_index = month - 1;
    let normalized_year = year + month_index.div_euclid(12);
    let normalized_month = month_index.rem_euclid(12) as u32 + 1;
    NaiveDate::from_ymd_opt(normalized_year, normalized_month, 1)?
        .checked_add_signed(ChronoDuration::days(i64::from(day - 1)))
}
/// 计算周序号。
fn week_number(core: CoreTime, monday: bool, one_based: bool) -> u32 {
    let Some(date) = NaiveDate::from_ymd_opt(core.Year(), core.Month() as u32, core.Day() as u32)
    else {
        return 0;
    };
    if monday && one_based {
        date.iso_week().week()
    } else {
        let format = if monday { "%W" } else { "%U" };
        date.format(format).to_string().parse::<u32>().unwrap_or(0) + u32::from(one_based)
    }
}
const WEEK_BEHAVIOUR_MONDAY_FIRST: u32 = 1 << 0;
const WEEK_BEHAVIOUR_YEAR: u32 = 1 << 1;
const WEEK_BEHAVIOUR_FIRST_WEEKDAY: u32 = 1 << 2;

/// MySQL WEEK 函数。
fn mysql_week(core: CoreTime, mode: i32) -> i32 {
    if core.Month() == 0 || core.Day() == 0 {
        return 0;
    }
    calc_mysql_week(core, week_mode(mode)).1
}

/// MySQL YEARWEEK：返回（年, 周）。
fn mysql_year_week(core: CoreTime, mode: i32) -> (i32, i32) {
    calc_mysql_week(core, week_mode(mode) | WEEK_BEHAVIOUR_YEAR)
}

/// YEARWEEK 年部分的定宽格式。
fn format_mysql_week_year(year: i32) -> String {
    if year < 0 {
        u32::MAX.to_string()
    } else {
        format!("{year:04}")
    }
}

/// 规范化 WEEK mode 位标志。
fn week_mode(mode: i32) -> u32 {
    let mut format = (mode & 7) as u32;
    if format & WEEK_BEHAVIOUR_MONDAY_FIRST == 0 {
        format ^= WEEK_BEHAVIOUR_FIRST_WEEKDAY;
    }
    format
}

/// 按 behaviour 位计算（年, 周）。
fn calc_mysql_week(core: CoreTime, behaviour: u32) -> (i32, i32) {
    let (year, month, day) = (core.Year(), core.Month(), core.Day());
    let daynr = calc_daynr(year, month, day);
    let mut first_daynr = calc_daynr(year, 1, 1);
    let monday_first = behaviour & WEEK_BEHAVIOUR_MONDAY_FIRST != 0;
    let mut week_year = behaviour & WEEK_BEHAVIOUR_YEAR != 0;
    let first_weekday = behaviour & WEEK_BEHAVIOUR_FIRST_WEEKDAY != 0;
    let mut weekday = calc_mysql_weekday(first_daynr, !monday_first);
    let mut result_year = year;
    let mut days;

    if month == 1 && day <= 7 - weekday {
        if !week_year && ((first_weekday && weekday != 0) || (!first_weekday && weekday >= 4)) {
            return (result_year, 0);
        }
        week_year = true;
        result_year -= 1;
        days = calc_days_in_year(result_year);
        first_daynr -= i64::from(days);
        weekday = (weekday + 53 * 7 - days) % 7;
    }

    if (first_weekday && weekday != 0) || (!first_weekday && weekday >= 4) {
        days = (daynr - (first_daynr + i64::from(7 - weekday))) as i32;
    } else {
        days = (daynr - (first_daynr - i64::from(weekday))) as i32;
    }

    if week_year && days >= 52 * 7 {
        weekday = (weekday + calc_days_in_year(result_year)) % 7;
        if (!first_weekday && weekday < 4) || (first_weekday && weekday == 0) {
            return (result_year + 1, 1);
        }
    }
    (result_year, days / 7 + 1)
}

/// 一年天数。
fn calc_days_in_year(year: i32) -> i32 {
    if (year & 3) == 0 && (year % 100 != 0 || (year % 400 == 0 && year != 0)) {
        366
    } else {
        365
    }
}

/// MySQL 风格星期几编号。
fn calc_mysql_weekday(daynr: i64, sunday_first: bool) -> i32 {
    ((daynr + 5 + i64::from(sunday_first)) % 7) as i32
}

/// 英文星期全名。
pub static WeekdayNames: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
/// 英文月份全名。
pub static MonthNames: [&str; 12] = [
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
const MONTH_ABBREV: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const ABBREV_WEEKDAY: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

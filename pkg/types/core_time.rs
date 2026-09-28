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

// CoreTime：MySQL 时间的 uint64 压缩表示与日期运算。
//
// 对照 Go `pkg/types/core_time.go`，保留 bit-field 编解码、日期差/周数、
// TIMESTAMPDIFF 以及与 Go `time.AddDate` 月末规则差异的修正。

// 本文件对照 pkg/types/core_time.go，保留 Time 的压缩表示、日期计算和
// TIMESTAMPDIFF 规则。外层 package harness 提供 Go time 对应的 gotime 适配。

use crate::errors;

// CoreTime 对应 Go 的 uint64 压缩时间结构，年月日时分秒微秒由 time.go 中的 bit mask 常量编码。
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct CoreTime(pub u64);

// ZeroCoreTime 对应 Go 的零值 TimeInternal。
pub const ZeroCoreTime: CoreTime = CoreTime(0);

impl CoreTime {
    // String implements fmt.Stringer.
    pub fn String(self) -> String {
        format!(
            "{{{} {} {} {} {} {} {}}}",
            self.getYear(),
            self.getMonth(),
            self.getDay(),
            self.getHour(),
            self.getMinute(),
            self.getSecond(),
            self.getMicrosecond()
        )
    }

    // getYear 从压缩 bit field 中取年份。
    pub fn getYear(self) -> u16 {
        ((self.0 & yearBitFieldMask) >> yearBitFieldOffset) as u16
    }

    // setYear 清除原年份位再写入新值；Go 通过 unsafe 指针直接改 uint64。
    pub fn setYear(&mut self, year: u16) {
        self.0 &= !yearBitFieldMask;
        self.0 |= ((year as u64) << yearBitFieldOffset) & yearBitFieldMask;
    }

    // Year returns the year value.
    pub fn Year(self) -> i32 {
        self.getYear() as i32
    }

    // getMonth 从压缩 bit field 中取月份。
    pub fn getMonth(self) -> u8 {
        ((self.0 & monthBitFieldMask) >> monthBitFieldOffset) as u8
    }

    // setMonth 清除原月份位再写入新值。
    pub fn setMonth(&mut self, month: u8) {
        self.0 &= !monthBitFieldMask;
        self.0 |= ((month as u64) << monthBitFieldOffset) & monthBitFieldMask;
    }

    // Month returns the month value.
    pub fn Month(self) -> i32 {
        self.getMonth() as i32
    }

    // getDay 从压缩 bit field 中取日。
    pub fn getDay(self) -> u8 {
        ((self.0 & dayBitFieldMask) >> dayBitFieldOffset) as u8
    }

    // setDay 清除原日字段再写入新值。
    pub fn setDay(&mut self, day: u8) {
        self.0 &= !dayBitFieldMask;
        self.0 |= ((day as u64) << dayBitFieldOffset) & dayBitFieldMask;
    }

    // Day returns the day value.
    pub fn Day(self) -> i32 {
        self.getDay() as i32
    }

    // getHour 从压缩 bit field 中取小时。
    pub fn getHour(self) -> u8 {
        ((self.0 & hourBitFieldMask) >> hourBitFieldOffset) as u8
    }

    // setHour 清除原小时字段再写入新值。
    pub fn setHour(&mut self, hour: u8) {
        self.0 &= !hourBitFieldMask;
        self.0 |= ((hour as u64) << hourBitFieldOffset) & hourBitFieldMask;
    }

    // Hour returns the hour value.
    pub fn Hour(self) -> i32 {
        self.getHour() as i32
    }

    // getMinute 从压缩 bit field 中取分钟。
    pub fn getMinute(self) -> u8 {
        ((self.0 & minuteBitFieldMask) >> minuteBitFieldOffset) as u8
    }

    // setMinute 清除原分钟字段再写入新值。
    pub fn setMinute(&mut self, minute: u8) {
        self.0 &= !minuteBitFieldMask;
        self.0 |= ((minute as u64) << minuteBitFieldOffset) & minuteBitFieldMask;
    }

    // Minute returns the minute value.
    pub fn Minute(self) -> i32 {
        self.getMinute() as i32
    }

    // getSecond 从压缩 bit field 中取秒。
    pub fn getSecond(self) -> u8 {
        ((self.0 & secondBitFieldMask) >> secondBitFieldOffset) as u8
    }

    // setSecond 清除原秒字段再写入新值。
    pub fn setSecond(&mut self, second: u8) {
        self.0 &= !secondBitFieldMask;
        self.0 |= ((second as u64) << secondBitFieldOffset) & secondBitFieldMask;
    }

    // Second returns the second value.
    pub fn Second(self) -> i32 {
        self.getSecond() as i32
    }

    // getMicrosecond 从压缩 bit field 中取微秒。
    pub fn getMicrosecond(self) -> u32 {
        ((self.0 & microsecondBitFieldMask) >> microsecondBitFieldOffset) as u32
    }

    // setMicrosecond 清除原微秒字段再写入新值。
    pub fn setMicrosecond(&mut self, microsecond: u32) {
        self.0 &= !microsecondBitFieldMask;
        self.0 |= ((microsecond as u64) << microsecondBitFieldOffset) & microsecondBitFieldMask;
    }

    // Microsecond returns the microsecond value.
    pub fn Microsecond(self) -> i32 {
        self.getMicrosecond() as i32
    }

    // Weekday returns weekday value.
    pub fn Weekday(self) -> gotime::Weekday {
        // Go 不考虑时区，直接用日期生成 UTC 时间；无效日期也允许从返回的 tm 取 Weekday。
        let (t1, _err) = self.GoTime(gotime::UTC);
        t1.Weekday()
    }

    // YearWeek returns year and week.
    pub fn YearWeek(self, mode: i32) -> (i32, i32) {
        let behavior = weekMode(mode) | weekBehaviourYear;
        calcWeek(self, behavior)
    }

    // Week returns week value.
    pub fn Week(self, mode: i32) -> i32 {
        if self.getMonth() == 0 || self.getDay() == 0 {
            return 0;
        }
        let (_, week) = calcWeek(self, weekMode(mode));
        week
    }

    // YearDay returns year and day.
    pub fn YearDay(self) -> i32 {
        if self.getMonth() == 0 || self.getDay() == 0 {
            return 0;
        }
        let (year, month, day) = (self.Year(), self.Month(), self.Day());
        calcDaynr(year, month, day) - calcDaynr(year, 1, 1) + 1
    }

    // GoTime converts Time to GoTime.
    pub fn GoTime(self, loc: gotime::Location) -> (gotime::Time, Option<errors::SharedError>) {
        let (year, month, day, hour, minute, second, microsecond) = (
            self.Year(),
            self.Month(),
            self.Day(),
            self.Hour(),
            self.Minute(),
            self.Second(),
            self.Microsecond(),
        );
        let tm = gotime::Date(
            year,
            gotime::Month(month),
            day,
            hour,
            minute,
            second,
            microsecond * 1000,
            loc,
        );
        let (year2, month2, day2) = tm.Date();
        let (hour2, minute2, second2) = tm.Clock();
        let microsec2 = tm.Nanosecond() / 1000;
        if year2 != year
            || month2 as i32 != month
            || day2 != day
            || hour2 != hour
            || minute2 != minute
            || second2 != second
            || microsec2 != microsecond
        {
            // Go 会返回转换后的 tm，同时返回 ErrWrongValue，表示输入日期不能被 gotime 精确表示。
            return (
                tm,
                Some(errors::New(format!(
                    "incorrect time value: {}",
                    self.String()
                ))),
            );
        }
        (tm, None)
    }

    // AdjustedGoTime converts Time to GoTime and adjust for invalid DST times.
    pub fn AdjustedGoTime(
        self,
        loc: gotime::Location,
    ) -> (gotime::Time, Option<errors::SharedError>) {
        let (tm, err) = self.GoTime(loc);
        if err.is_none() {
            return (tm, None);
        }

        // Go 判断 gotime 没能映回原时间时，尝试调整到最近的 DST 边界。
        let (start, end) = tm.ZoneBounds();
        if start.Sub(tm).Abs().Hours() > 4.0 && end.Sub(tm).Abs().Hours() > 4.0 {
            return (
                tm,
                Some(errors::New(
                    "incorrect time value after timezone conversion",
                )),
            );
        }
        if tm.Sub(start).Abs() <= tm.Sub(end).Abs() {
            return (start, None);
        }
        (end, None)
    }

    // IsLeapYear returns if it's leap year.
    pub fn IsLeapYear(self) -> bool {
        isLeapYear(self.getYear())
    }
}

// isLeapYear 对应 Go 的闰年判断。
pub fn isLeapYear(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 各月天数表（非闰年 2 月为 28）。
pub const daysByMonth: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

// GetLastDay returns the last day of the month.
pub fn GetLastDay(year: i32, month: i32) -> i32 {
    let mut day = 0;
    if month > 0 && month <= 12 {
        day = daysByMonth[(month - 1) as usize];
    }
    if month == 2 && isLeapYear(year as u16) {
        day = 29;
    }
    day
}

// getFixDays 修正 Go time.AddDate 与 MySQL 月末加减月份规则的差异。
pub fn getFixDays(year: i32, month: i32, day: i32, ot: gotime::Time) -> i32 {
    if (year != 0 || month != 0) && day == 0 {
        let od = ot.Day();
        let t = ot.AddDate(year, month, day);
        let td = t.Day();
        if od != td {
            let tm = t.Month() as i32 - 1;
            let tMax = GetLastDay(t.Year(), tm);
            let dd = tMax - od;
            return dd;
        }
    }
    0
}

// compareTime compare two Time.
pub fn compareTime(a: CoreTime, b: CoreTime) -> i32 {
    let ta = datetimeToUint64(a);
    let tb = datetimeToUint64(b);

    if ta < tb {
        return -1;
    } else if ta > tb {
        return 1;
    }

    if a.Microsecond() < b.Microsecond() {
        return -1;
    } else if a.Microsecond() > b.Microsecond() {
        return 1;
    }

    0
}

// AddDate fix gap between mysql and golang api.
pub fn AddDate(
    year: i64,
    month: i64,
    day: i64,
    ot: gotime::Time,
) -> (gotime::Time, Option<errors::SharedError>) {
    // 限制加减范围，避免 gotime.AddDate 内部溢出；范围沿用 Go 的 10000*365。
    const maxAdd: i64 = 10000 * 365;
    const minAdd: i64 = -maxAdd;
    let mut nt = gotime::Time::default();
    if year > maxAdd
        || year < minAdd
        || month > maxAdd
        || month < minAdd
        || day > maxAdd
        || day < minAdd
    {
        return (nt, Some(errors::New("datetime function overflow")));
    }

    let df = getFixDays(year as i32, month as i32, day as i32, ot);
    if df != 0 {
        nt = ot.AddDate(year as i32, month as i32, df);
    } else {
        nt = ot.AddDate(year as i32, month as i32, day as i32);
    }

    if nt.Year() < 0 || nt.Year() > 9999 {
        return (nt, Some(errors::New("datetime function overflow")));
    }

    (nt, None)
}

// calcTimeFromSec 把秒数拆回 CoreTime 的时分秒微秒字段。
pub fn calcTimeFromSec(to: &mut CoreTime, mut seconds: i64, microseconds: i64) {
    to.setHour((seconds / 3600) as u8);
    seconds %= 3600;
    to.setMinute((seconds / 60) as u8);
    to.setSecond((seconds % 60) as u8);
    to.setMicrosecond(microseconds as u32);
}

/// 一天的秒数。
pub const secondsIn24Hour: i64 = 86_400;

// calcTimeDiffInternal 对应 Go 的内部差值计算，返回绝对秒数、微秒和原差值是否为负。
pub fn calcTimeDiffInternal(
    t1: CoreTime,
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    sign: i32,
) -> (i64, i64, bool) {
    let mut days = i64::from(calcDaynr(t1.Year(), t1.Month(), t1.Day()));
    let days2 = i64::from(calcDaynr(year, month, day));
    days -= i64::from(sign) * days2;

    let mut tmp = (days * secondsIn24Hour
        + t1.Hour() as i64 * 3600
        + t1.Minute() as i64 * 60
        + t1.Second() as i64
        - sign as i64 * (hour as i64 * 3600 + minute as i64 * 60 + second as i64))
        * 1_000_000
        + t1.Microsecond() as i64
        - sign as i64 * microsecond as i64;

    let mut neg = false;
    if tmp < 0 {
        tmp = -tmp;
        neg = true;
    }
    (tmp / 1_000_000, tmp % 1_000_000, neg)
}

// calcTimeTimeDiff calculates difference between two datetime values as seconds + microseconds.
pub fn calcTimeTimeDiff(t1: CoreTime, t2: CoreTime, sign: i32) -> (i64, i64, bool) {
    calcTimeDiffInternal(
        t1,
        t2.Year(),
        t2.Month(),
        t2.Day(),
        t2.Hour(),
        t2.Minute(),
        t2.Second(),
        t2.Microsecond(),
        sign,
    )
}

// calcTimeDurationDiff calculates difference between a datetime value and a duration as seconds + microseconds.
pub fn calcTimeDurationDiff(t: CoreTime, d: Duration) -> (i64, i64, bool) {
    let (sign, hh, mm, ss, micro) = splitDuration(d.Duration);
    calcTimeDiffInternal(t, 0, 0, 0, hh, mm, ss, micro, -sign)
}

// datetimeToUint64 converts time value to integer in YYYYMMDDHHMMSS format.
pub fn datetimeToUint64(t: CoreTime) -> u64 {
    t.Year() as u64 * 10_000_000_000
        + t.Month() as u64 * 100_000_000
        + t.Day() as u64 * 1_000_000
        + t.Hour() as u64 * 10_000
        + t.Minute() as u64 * 100
        + t.Second() as u64
}

// calcDaynr calculates days since 0000-00-00.
pub fn calcDaynr(mut year: i32, month: i32, day: i32) -> i32 {
    if year == 0 && month == 0 {
        return 0;
    }

    let mut delsum = 365 * year + 31 * (month - 1) + day;
    if month <= 2 {
        year -= 1;
    } else {
        delsum -= (month * 4 + 23) / 10;
    }
    let temp = ((year / 100 + 1) * 3) / 4;
    delsum + year / 4 - temp
}

// DateDiff calculates number of days between two days.
pub fn DateDiff(startTime: CoreTime, endTime: CoreTime) -> i32 {
    calcDaynr(startTime.Year(), startTime.Month(), startTime.Day())
        - calcDaynr(endTime.Year(), endTime.Month(), endTime.Day())
}

// calcDaysInYear calculates days in one year, it works with 0 <= year <= 99.
pub fn calcDaysInYear(year: i32) -> i32 {
    if (year & 3) == 0 && (year % 100 != 0 || (year % 400 == 0 && year != 0)) {
        return 366;
    }
    365
}

// calcWeekday calculates weekday from daynr, returns 0 for Monday, 1 for Tuesday ...
pub fn calcWeekday(mut daynr: i32, sundayFirstDayOfWeek: bool) -> i32 {
    daynr += 5;
    if sundayFirstDayOfWeek {
        daynr += 1;
    }
    daynr % 7
}

// weekBehaviour 对应 Go 的 bit flags，控制 WEEK/YEARWEEK 计算模式。
pub type weekBehaviour = u32;

/// 周一作为一周第一天。
pub const weekBehaviourMondayFirst: weekBehaviour = 1 << 0;
/// 返回周所属年份（与周数一并）。
pub const weekBehaviourYear: weekBehaviour = 1 << 1;
/// 要求第一周包含本周第一个工作日。
pub const weekBehaviourFirstWeekday: weekBehaviour = 1 << 2;

// weekBehaviour.test 对应 Go 方法：检查某个模式位是否设置。
pub fn weekBehaviourTest(v: weekBehaviour, flag: weekBehaviour) -> bool {
    (v & flag) != 0
}

// weekMode 保留 MySQL mode 低三位映射，并在周日优先时翻转 FirstWeekday。
pub fn weekMode(mode: i32) -> weekBehaviour {
    let mut weekFormat = (mode & 7) as weekBehaviour;
    if (weekFormat & weekBehaviourMondayFirst) == 0 {
        weekFormat ^= weekBehaviourFirstWeekday;
    }
    weekFormat
}

// calcWeek calculates week and year for the time.
pub fn calcWeek(t: CoreTime, wb: weekBehaviour) -> (i32, i32) {
    let mut days: i32;
    let (ty, tm, td) = (t.getYear() as i32, t.getMonth() as i32, t.getDay() as i32);
    let daynr = calcDaynr(ty, tm, td);
    let mut firstDaynr = calcDaynr(ty, 1, 1);
    let mondayFirst = weekBehaviourTest(wb, weekBehaviourMondayFirst);
    let mut weekYear = weekBehaviourTest(wb, weekBehaviourYear);
    let firstWeekday = weekBehaviourTest(wb, weekBehaviourFirstWeekday);

    let mut weekday = calcWeekday(firstDaynr, !mondayFirst);
    let mut year = ty;
    if tm == 1 && td <= 7 - weekday {
        if !weekYear && ((firstWeekday && weekday != 0) || (!firstWeekday && weekday >= 4)) {
            return (year, 0);
        }
        weekYear = true;
        year -= 1;
        days = calcDaysInYear(year);
        firstDaynr -= days;
        weekday = (weekday + 53 * 7 - days) % 7;
    }

    if (firstWeekday && weekday != 0) || (!firstWeekday && weekday >= 4) {
        days = daynr - (firstDaynr + 7 - weekday);
    } else {
        days = daynr - (firstDaynr - weekday);
    }

    if weekYear && days >= 52 * 7 {
        weekday = (weekday + calcDaysInYear(year)) % 7;
        if (!firstWeekday && weekday < 4) || (firstWeekday && weekday == 0) {
            year += 1;
            return (year, 1);
        }
    }
    (year, days / 7 + 1)
}

// mixDateAndDuration mixes a date value and a duration value.
pub fn mixDateAndDuration(date: &mut CoreTime, dur: Duration) {
    if dur.Duration >= 0 && dur.Hour() < 24 {
        let (_, hh, mm, ss, frac) = splitDuration(dur.Duration);
        date.setHour(hh as u8);
        date.setMinute(mm as u8);
        date.setSecond(ss as u8);
        date.setMicrosecond(frac as u32);
        return;
    }

    // 负 duration 或超过 24 小时的时间需要换算到 daynr 再拆回日期。
    let (seconds, microseconds, _) = calcTimeDurationDiff(*date, dur);
    let days = seconds / secondsIn24Hour;
    calcTimeFromSec(date, seconds % secondsIn24Hour, microseconds);
    let (year, month, day) = getDateFromDaynr(days as u32);
    date.setYear(year as u16);
    date.setMonth(month as u8);
    date.setDay(day as u8);
}

// getDateFromDaynr changes a daynr to year, month and day; daynr 0 returns 00.00.00.
pub fn getDateFromDaynr(daynr: u32) -> (u32, u32, u32) {
    if daynr <= 365 || daynr >= 3652500 {
        return (0, 0, 0);
    }

    let mut year = daynr * 100 / 36525;
    let temp = (((year - 1) / 100 + 1) * 3) / 4;
    let mut dayOfYear = daynr - year * 365 - (year - 1) / 4 + temp;

    let mut daysInYear = calcDaysInYear(year as i32);
    while dayOfYear > daysInYear as u32 {
        dayOfYear -= daysInYear as u32;
        year += 1;
        daysInYear = calcDaysInYear(year as i32);
    }

    let mut leapDay = 0_u32;
    if daysInYear == 366 && dayOfYear > 31 + 28 {
        dayOfYear -= 1;
        if dayOfYear == 31 + 28 {
            // 这里处理闰年的 2 月 29 日。
            leapDay = 1;
        }
    }

    let mut month = 1_u32;
    for days in daysByMonth {
        if dayOfYear <= days as u32 {
            break;
        }
        dayOfYear -= days as u32;
        month += 1;
    }

    let day = dayOfYear + leapDay;
    (year, month, day)
}

/// TIMESTAMPDIFF 单位：年。
pub const intervalYEAR: &str = "YEAR";
/// TIMESTAMPDIFF 单位：季。
pub const intervalQUARTER: &str = "QUARTER";
/// TIMESTAMPDIFF 单位：月。
pub const intervalMONTH: &str = "MONTH";
/// TIMESTAMPDIFF 单位：周。
pub const intervalWEEK: &str = "WEEK";
/// TIMESTAMPDIFF 单位：日。
pub const intervalDAY: &str = "DAY";
/// TIMESTAMPDIFF 单位：时。
pub const intervalHOUR: &str = "HOUR";
/// TIMESTAMPDIFF 单位：分。
pub const intervalMINUTE: &str = "MINUTE";
/// TIMESTAMPDIFF 单位：秒。
pub const intervalSECOND: &str = "SECOND";
/// TIMESTAMPDIFF 单位：微秒。
pub const intervalMICROSECOND: &str = "MICROSECOND";

// timestampDiff 对应 Go 的 TIMESTAMPDIFF 计算，按 intervalType 返回年、季度、月或秒级差值。
pub fn timestampDiff(intervalType: &str, t1: CoreTime, t2: CoreTime) -> i64 {
    let (seconds, microseconds, neg) = calcTimeTimeDiff(t2, t1, 1);
    let mut months = 0_u32;
    if intervalType == intervalYEAR
        || intervalType == intervalQUARTER
        || intervalType == intervalMONTH
    {
        let (
            yearBeg,
            yearEnd,
            monthBeg,
            monthEnd,
            dayBeg,
            dayEnd,
            secondBeg,
            secondEnd,
            microsecondBeg,
            microsecondEnd,
        ) = if neg {
            (
                t2.Year() as u32,
                t1.Year() as u32,
                t2.Month() as u32,
                t1.Month() as u32,
                t2.Day() as u32,
                t1.Day() as u32,
                (t2.Hour() * 3600 + t2.Minute() * 60 + t2.Second()) as u32,
                (t1.Hour() * 3600 + t1.Minute() * 60 + t1.Second()) as u32,
                t2.Microsecond() as u32,
                t1.Microsecond() as u32,
            )
        } else {
            (
                t1.Year() as u32,
                t2.Year() as u32,
                t1.Month() as u32,
                t2.Month() as u32,
                t1.Day() as u32,
                t2.Day() as u32,
                (t1.Hour() * 3600 + t1.Minute() * 60 + t1.Second()) as u32,
                (t2.Hour() * 3600 + t2.Minute() * 60 + t2.Second()) as u32,
                t1.Microsecond() as u32,
                t2.Microsecond() as u32,
            )
        };

        // 年份差先按月日修正，随后再折算月份差。
        let mut years = yearEnd - yearBeg;
        if monthEnd < monthBeg || (monthEnd == monthBeg && dayEnd < dayBeg) {
            years -= 1;
        }

        months = 12 * years;
        if monthEnd < monthBeg || (monthEnd == monthBeg && dayEnd < dayBeg) {
            months += 12 - (monthBeg - monthEnd);
        } else {
            months += monthEnd - monthBeg;
        }

        if dayEnd < dayBeg {
            months -= 1;
        } else if dayEnd == dayBeg
            && (secondEnd < secondBeg
                || (secondEnd == secondBeg && microsecondEnd < microsecondBeg))
        {
            months -= 1;
        }
    }

    let negV = if neg { -1_i64 } else { 1_i64 };
    match intervalType {
        intervalYEAR => months as i64 / 12 * negV,
        intervalQUARTER => months as i64 / 3 * negV,
        intervalMONTH => months as i64 * negV,
        intervalWEEK => seconds / secondsIn24Hour / 7 * negV,
        intervalDAY => seconds / secondsIn24Hour * negV,
        intervalHOUR => seconds / 3600 * negV,
        intervalMINUTE => seconds / 60 * negV,
        intervalSECOND => seconds * negV,
        intervalMICROSECOND => {
            // MySQL 任意两个有效 datetime 的微秒差可放入 longlong。
            (seconds * 1_000_000 + microseconds) * negV
        }
        _ => 0,
    }
}

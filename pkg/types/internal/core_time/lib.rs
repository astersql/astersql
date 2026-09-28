// Copyright 2026 AsterSQL.
// Formal implementation owner: pkg/types.

// CoreTime 内部 crate：Go 风格时间与压缩时间戳位域。
//
// `gotime` 用 chrono 模拟 Go `time.Time`；`types` 提供 CoreTime 位打包、
// Datum 桩与 `core_time`/`datum_eval` 实现挂接。正式实现归属 `pkg/types`。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 错误类型再导出。
pub use astersql_errors as errors;

/// 近似 Go `time` 包的日历时间与时长工具。
pub mod gotime {
    use chrono::{
        Datelike, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike,
    };
    use chrono_tz::{GapInfo, Tz};
    use std::{cmp::Ordering, fmt, str::FromStr};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    /// Go `time.Location` 的命名时区/固定偏移适配。
    pub enum Location {
        /// IANA 命名时区。
        Named(Tz),
        /// 无跳变的固定 UTC 偏移。
        Fixed {
            name: &'static str,
            offset_seconds: i32,
        },
    }

    impl Default for Location {
        fn default() -> Self {
            UTC
        }
    }

    impl From<Tz> for Location {
        fn from(value: Tz) -> Self {
            Self::Named(value)
        }
    }

    /// UTC 时区常量。
    pub const UTC: Location = Location::Named(chrono_tz::UTC);

    /// 构造固定偏移时区，对齐 Go `time.FixedZone`。
    pub const fn FixedZone(name: &'static str, offset_seconds: i32) -> Location {
        Location::Fixed {
            name,
            offset_seconds,
        }
    }

    /// 按 IANA 名称加载时区，对齐 Go `time.LoadLocation`。
    pub fn LoadLocation(name: &str) -> Result<Location, String> {
        Tz::from_str(name)
            .map(Location::Named)
            .map_err(|error| format!("unknown time zone {name}: {error}"))
    }

    #[derive(Clone, Copy, Debug)]
    /// 带命名时区或固定偏移的绝对时间点。
    pub struct Time {
        utc: NaiveDateTime,
        location: Location,
    }

    impl PartialEq for Time {
        fn eq(&self, other: &Self) -> bool {
            self.utc == other.utc
        }
    }

    impl Eq for Time {}

    impl PartialOrd for Time {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            self.utc.partial_cmp(&other.utc)
        }
    }

    impl Default for Time {
        fn default() -> Self {
            let naive = NaiveDate::from_ymd_opt(1, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            Self {
                utc: naive,
                location: UTC,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    /// 星期枚举（周日为一周起点，对齐 Go）。
    pub enum Weekday {
        Sunday,
        Monday,
        Tuesday,
        Wednesday,
        Thursday,
        Friday,
        Saturday,
    }

    impl fmt::Display for Weekday {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{self:?}")
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd)]
    /// 时长，内部单位为纳秒，对齐 Go `time.Duration`。
    pub struct Duration(i64);

    impl Duration {
        /// 取绝对值时长。
        pub fn Abs(self) -> Self {
            Self(self.0.saturating_abs())
        }

        /// 换算为小时（浮点）。
        pub fn Hours(self) -> f64 {
            self.0 as f64 / 3_600_000_000_000.0
        }
    }

    #[allow(non_snake_case)]
    /// 透传月份值（兼容 Go 调用形态）。
    pub fn Month(month: i32) -> i32 {
        month
    }

    #[allow(clippy::too_many_arguments)]
    /// 由年月日时分秒纳秒构造 Time；月份可越界归一化。
    pub fn Date(
        year: i32,
        month: i32,
        day: i32,
        hour: i32,
        minute: i32,
        second: i32,
        nanosecond: i32,
        location: Location,
    ) -> Time {
        let month_index = i64::from(year) * 12 + i64::from(month) - 1;
        let normalized_year = month_index.div_euclid(12) as i32;
        let normalized_month = month_index.rem_euclid(12) as u32 + 1;
        let base = NaiveDate::from_ymd_opt(normalized_year, normalized_month, 1)
            .expect("normalized date is in chrono range")
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let local = base
            + chrono::Duration::days(i64::from(day) - 1)
            + chrono::Duration::hours(i64::from(hour))
            + chrono::Duration::minutes(i64::from(minute))
            + chrono::Duration::seconds(i64::from(second))
            + chrono::Duration::nanoseconds(i64::from(nanosecond));
        let utc = match location {
            Location::Named(timezone) => match timezone.from_local_datetime(&local) {
                LocalResult::Single(value) => value.naive_utc(),
                // Go 在时钟回拨的重叠区间选择较晚（标准时）实例。
                LocalResult::Ambiguous(_, latest) => latest.naive_utc(),
                LocalResult::None => {
                    // Go time.Date 用跳变前偏移解释 DST 空洞中的墙上时间。
                    // 转换后的墙上时间会落到空洞之前，GoTime 随后据此报告不一致，
                    // AdjustedGoTime 再选择最近的真实时区边界。
                    let gap = GapInfo::new(&local, &timezone)
                        .expect("chrono-tz reports a gap without transition metadata");
                    let (_, offset_before) = gap
                        .begin
                        .expect("timezone gap has no preceding fixed offset");
                    offset_before
                        .fix()
                        .from_local_datetime(&local)
                        .single()
                        .unwrap()
                        .naive_utc()
                }
            },
            Location::Fixed { offset_seconds, .. } => FixedOffset::east_opt(offset_seconds)
                .expect("fixed-zone offset is outside chrono range")
                .from_local_datetime(&local)
                .single()
                .unwrap()
                .naive_utc(),
        };
        Time { utc, location }
    }

    fn local_datetime(time: Time) -> NaiveDateTime {
        match time.location {
            Location::Named(timezone) => timezone.from_utc_datetime(&time.utc).naive_local(),
            Location::Fixed { offset_seconds, .. } => {
                time.utc + chrono::Duration::seconds(i64::from(offset_seconds))
            }
        }
    }

    fn instant_after(time: Time) -> Option<jiff::Timestamp> {
        let utc = time.utc.and_utc();
        let seconds = utc.timestamp();
        let nanoseconds = utc.timestamp_subsec_nanos() as i32;
        if nanoseconds == 999_999_999 {
            jiff::Timestamp::new(seconds.checked_add(1)?, 0).ok()
        } else {
            jiff::Timestamp::new(seconds, nanoseconds + 1).ok()
        }
    }

    fn transition_time(location: Location, timestamp: jiff::Timestamp) -> Option<Time> {
        Some(Time {
            utc: chrono::DateTime::from_timestamp(
                timestamp.as_second(),
                timestamp.subsec_nanosecond() as u32,
            )?
            .naive_utc(),
            location,
        })
    }

    fn transition_before(time: Time) -> Option<Time> {
        let Location::Named(timezone) = time.location else {
            return None;
        };
        let timezone_rules = jiff::tz::TimeZone::get(timezone.name()).ok()?;
        // `preceding` 严格排除查询时刻；向后移 1ns 可包含恰好位于
        // 当前时刻的跳变，使返回值符合 ZoneBounds 的闭区间起点语义。
        let transition = timezone_rules.preceding(instant_after(time)?).next()?;
        transition_time(time.location, transition.timestamp())
    }

    fn transition_after(time: Time) -> Option<Time> {
        let Location::Named(timezone) = time.location else {
            return None;
        };
        let timezone_rules = jiff::tz::TimeZone::get(timezone.name()).ok()?;
        let utc = time.utc.and_utc();
        let timestamp =
            jiff::Timestamp::new(utc.timestamp(), utc.timestamp_subsec_nanos() as i32).ok()?;
        let transition = timezone_rules.following(timestamp).next()?;
        transition_time(time.location, transition.timestamp())
    }

    impl Time {
        /// 拆出年、月、日。
        pub fn Date(self) -> (i32, i32, i32) {
            let local = local_datetime(self);
            (local.year(), local.month() as i32, local.day() as i32)
        }

        /// 拆出时、分、秒。
        pub fn Clock(self) -> (i32, i32, i32) {
            let local = local_datetime(self);
            (
                local.hour() as i32,
                local.minute() as i32,
                local.second() as i32,
            )
        }

        /// 纳秒分量。
        pub fn Nanosecond(self) -> i32 {
            local_datetime(self).nanosecond() as i32
        }

        /// 返回当前时区缩写与 UTC 偏移秒数，对齐 Go `Time.Zone`。
        pub fn Zone(self) -> (String, i32) {
            match self.location {
                Location::Named(timezone) => {
                    let local = timezone.from_utc_datetime(&self.utc);
                    (
                        local.format("%Z").to_string(),
                        local.offset().fix().local_minus_utc(),
                    )
                }
                Location::Fixed {
                    name,
                    offset_seconds,
                } => (name.to_owned(), offset_seconds),
            }
        }

        /// 星期几。
        pub fn Weekday(self) -> Weekday {
            match local_datetime(self).weekday() {
                chrono::Weekday::Mon => Weekday::Monday,
                chrono::Weekday::Tue => Weekday::Tuesday,
                chrono::Weekday::Wed => Weekday::Wednesday,
                chrono::Weekday::Thu => Weekday::Thursday,
                chrono::Weekday::Fri => Weekday::Friday,
                chrono::Weekday::Sat => Weekday::Saturday,
                chrono::Weekday::Sun => Weekday::Sunday,
            }
        }

        /// 返回包含当前时刻的真实 IANA 时区规则起止边界。
        ///
        /// 对无界端点返回零值，和 Go `Time.ZoneBounds` 一致。
        pub fn ZoneBounds(self) -> (Time, Time) {
            (
                transition_before(self).unwrap_or_default(),
                transition_after(self).unwrap_or_default(),
            )
        }

        /// 两时间点之差；超出 Go Duration 范围时饱和。
        pub fn Sub(self, other: Time) -> Duration {
            let difference = self.utc.signed_duration_since(other.utc);
            Duration(difference.num_nanoseconds().unwrap_or_else(|| {
                if difference < chrono::Duration::zero() {
                    i64::MIN
                } else {
                    i64::MAX
                }
            }))
        }

        /// 按年月日增量偏移（经 Date 归一化）。
        pub fn AddDate(self, years: i32, months: i32, days: i32) -> Time {
            let (year, month, day) = self.Date();
            let (hour, minute, second) = self.Clock();
            Date(
                year + years,
                month + months,
                day + days,
                hour,
                minute,
                second,
                self.Nanosecond(),
                self.location,
            )
        }

        /// 日（1..=31）。
        pub fn Day(self) -> i32 {
            local_datetime(self).day() as i32
        }

        /// 月（1..=12）。
        pub fn Month(self) -> i32 {
            local_datetime(self).month() as i32
        }

        /// 年。
        pub fn Year(self) -> i32 {
            local_datetime(self).year()
        }
    }
}

/// CoreTime 位域、Datum 桩与生产实现 include 入口。
pub mod types {
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    /// 测试桩：用 BigDecimal 充当 MyDecimal。
    pub type MyDecimal = BigDecimal;

    /// CoreTime u64 中各日历字段的位偏移与宽度。
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

    pub const yearBitFieldMask: u64 = ((1 << yearBitFieldWidth) - 1) << yearBitFieldOffset;
    pub const monthBitFieldMask: u64 = ((1 << monthBitFieldWidth) - 1) << monthBitFieldOffset;
    pub const dayBitFieldMask: u64 = ((1 << dayBitFieldWidth) - 1) << dayBitFieldOffset;
    pub const hourBitFieldMask: u64 = ((1 << hourBitFieldWidth) - 1) << hourBitFieldOffset;
    pub const minuteBitFieldMask: u64 = ((1 << minuteBitFieldWidth) - 1) << minuteBitFieldOffset;
    pub const secondBitFieldMask: u64 = ((1 << secondBitFieldWidth) - 1) << secondBitFieldOffset;
    pub const microsecondBitFieldMask: u64 =
        ((1 << microsecondBitFieldWidth) - 1) << microsecondBitFieldOffset;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    /// MySQL TIME 时长：微秒值 + 小数秒精度（FSP）。
    pub struct Duration {
        pub Duration: i64,
        pub Fsp: i32,
    }

    impl Duration {
        /// 整小时数（由微秒整除）。
        pub fn Hour(self) -> i32 {
            splitDuration(self.Duration).1
        }
    }

    /// 将微秒时长拆成 (符号, 时, 分, 秒, 微秒)。
    pub fn splitDuration(duration: i64) -> (i32, i32, i32, i32, i32) {
        let sign = if duration < 0 { -1 } else { 1 };
        let mut micros = duration.saturating_abs();
        let hour = (micros / 3_600_000_000) as i32;
        micros %= 3_600_000_000;
        let minute = (micros / 60_000_000) as i32;
        micros %= 60_000_000;
        let second = (micros / 1_000_000) as i32;
        let microsecond = (micros % 1_000_000) as i32;
        (sign, hour, minute, second, microsecond)
    }

    /// 将年月日时分秒微秒打包为 CoreTime 位域。
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
        value |= ((microsecond as u64) << microsecondBitFieldOffset) & microsecondBitFieldMask;
        value |= ((second as u64) << secondBitFieldOffset) & secondBitFieldMask;
        value |= ((minute as u64) << minuteBitFieldOffset) & minuteBitFieldMask;
        value |= ((hour as u64) << hourBitFieldOffset) & hourBitFieldMask;
        value |= ((day as u64) << dayBitFieldOffset) & dayBitFieldMask;
        value |= ((month as u64) << monthBitFieldOffset) & monthBitFieldMask;
        value |= ((year as u64) << yearBitFieldOffset) & yearBitFieldMask;
        CoreTime(value)
    }

    /// 数值溢出辅助。
    pub mod overflow {
        include!("../../overflow.rs");
    }

    #[derive(Clone, Debug, PartialEq)]
    /// Datum 内部存储变体。
    enum DatumValue {
        Null,
        Int64(i64),
        Uint64(u64),
        Float64(f64),
        Decimal(BigDecimal),
        String(String),
    }

    impl Default for DatumValue {
        fn default() -> Self {
            Self::Null
        }
    }

    /// Datum 类型标签常量。
    pub const KindNull: u8 = 0;
    pub const KindInt64: u8 = 1;
    pub const KindUint64: u8 = 2;
    pub const KindFloat64: u8 = 4;
    pub const KindString: u8 = 5;
    pub const KindMysqlDecimal: u8 = 8;

    #[derive(Clone, Debug, Default, PartialEq)]
    /// 标量值容器（测试/桩用简化版）。
    pub struct Datum {
        value: DatumValue,
        frac: i32,
    }

    impl Datum {
        /// 当前值的 Kind 标签。
        pub fn Kind(&self) -> u8 {
            match self.value {
                DatumValue::Null => KindNull,
                DatumValue::Int64(_) => KindInt64,
                DatumValue::Uint64(_) => KindUint64,
                DatumValue::Float64(_) => KindFloat64,
                DatumValue::Decimal(_) => KindMysqlDecimal,
                DatumValue::String(_) => KindString,
            }
        }

        /// 取有符号整数；非该 Kind 返回 0。
        pub fn GetInt64(&self) -> i64 {
            match self.value {
                DatumValue::Int64(value) => value,
                _ => 0,
            }
        }

        /// 写入有符号整数。
        pub fn SetInt64(&mut self, value: i64) {
            self.value = DatumValue::Int64(value);
        }

        /// 取无符号整数。
        pub fn GetUint64(&self) -> u64 {
            match self.value {
                DatumValue::Uint64(value) => value,
                _ => 0,
            }
        }

        /// 写入无符号整数。
        pub fn SetUint64(&mut self, value: u64) {
            self.value = DatumValue::Uint64(value);
        }

        /// 取浮点。
        pub fn GetFloat64(&self) -> f64 {
            match self.value {
                DatumValue::Float64(value) => value,
                _ => 0.0,
            }
        }

        /// 写入浮点。
        pub fn SetFloat64(&mut self, value: f64) {
            self.value = DatumValue::Float64(value);
        }

        /// 取 DECIMAL。
        pub fn GetMysqlDecimal(&self) -> BigDecimal {
            match &self.value {
                DatumValue::Decimal(value) => value.clone(),
                _ => BigDecimal::default(),
            }
        }

        /// 写入 DECIMAL。
        pub fn SetMysqlDecimal(&mut self, value: BigDecimal) {
            self.value = DatumValue::Decimal(value);
        }

        /// 小数位数。
        pub fn Frac(&self) -> i32 {
            self.frac
        }

        /// 设置小数位数。
        pub fn SetFrac(&mut self, frac: i32) {
            self.frac = frac;
        }

        /// 调试用字符串表示。
        pub fn GetValue(&self) -> String {
            match &self.value {
                DatumValue::Null => "<nil>".to_string(),
                DatumValue::Int64(value) => value.to_string(),
                DatumValue::Uint64(value) => value.to_string(),
                DatumValue::Float64(value) => value.to_string(),
                DatumValue::Decimal(value) => value.to_string(),
                DatumValue::String(value) => value.clone(),
            }
        }
    }

    /// 构造 Int64 Datum。
    pub fn NewIntDatum(value: i64) -> Datum {
        let mut datum = Datum::default();
        datum.SetInt64(value);
        datum
    }

    /// 构造 Uint64 Datum。
    pub fn NewUintDatum(value: u64) -> Datum {
        let mut datum = Datum::default();
        datum.SetUint64(value);
        datum
    }

    /// 构造 Float64 Datum。
    pub fn NewFloat64Datum(value: f64) -> Datum {
        let mut datum = Datum::default();
        datum.SetFloat64(value);
        datum
    }

    /// 构造 Decimal Datum。
    pub fn NewDecimalDatum(value: BigDecimal) -> Datum {
        let mut datum = Datum::default();
        datum.SetMysqlDecimal(value);
        datum
    }

    /// 构造 String Datum。
    pub fn NewStringDatum(value: &str) -> Datum {
        Datum {
            value: DatumValue::String(value.to_string()),
            frac: 0,
        }
    }

    /// 测试用：从字符串解析 BigDecimal。
    pub fn NewDecFromStringForTest(value: &str) -> BigDecimal {
        BigDecimal::from_str(value).unwrap()
    }

    /// DECIMAL 加法，结果写入 `result`。
    pub fn DecimalAdd(
        lhs: &BigDecimal,
        rhs: &BigDecimal,
        result: &mut BigDecimal,
    ) -> Result<(), crate::errors::SharedError> {
        *result = lhs + rhs;
        Ok(())
    }

    /// 二元运算符枚举桩。
    pub mod opcode {
        #[derive(Clone, Copy, Debug)]
        pub enum Op {
            Plus,
        }

        pub const Plus: Op = Op::Plus;
    }

    /// 非法运算错误构造（用于类型不匹配路径）。
    pub fn InvOp2(lhs: String, rhs: String, _op: opcode::Op) -> ((), Option<crate::errors::Error>) {
        (
            (),
            Some(crate::errors::Normalize(
                format!("Invalid operation: {lhs} + {rhs}"),
                &[],
            )),
        )
    }

    /// 挂接生产 CoreTime 实现。
    pub mod core_time {
        use super::*;
        use crate::gotime;
        include!("../../core_time.rs");
    }
    pub use core_time::*;

    /// 挂接 Datum 求值实现。
    pub mod datum_eval {
        use super::*;
        include!("../../datum_eval.rs");
    }
    pub use datum_eval::*;
}

/// 再导出 types 子模块公共 API。
pub use types::*;

#[cfg(test)]
#[path = "../../core_time_2_aster_unit_test.rs"]
/// 迁移期 CoreTime 单元测试。
mod core_time_2_aster_unit_test;

#[cfg(test)]
mod migration_aster_unit_test;

#[cfg(test)]
mod lib_aster_unit_test;

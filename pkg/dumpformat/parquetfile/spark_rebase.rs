// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Spark legacy Julian/Gregorian DATE/TIMESTAMP rebasing。
//
// 根据 footer metadata 判定是否走旧历法，再按生成表 switches/diffs 修正。
// rebase：把混合历法写入的时间点换算到现代公历（Gregorian）语义。
// limitations under the License.

// 读取 Spark 旧历法 Parquet 文件时，如何根据 footer metadata 和生成表完成 DATE/TIMESTAMP rebasing。
//
// Spark legacy rebase metadata 常量保持 Go 文件顺序。
// pub const sparkRebaseDefaultTimeZoneID: &str = "UTC";
// pub const sparkAppName: &str = "spark";
// pub const sparkVersionMetadataKey: &str = "org.apache.spark.version";
// pub const sparkTimeZoneMetadataKey: &str = "org.apache.spark.timeZone";
// pub const sparkLegacyDateTimeMetadataKey: &str = "org.apache.spark.legacyDateTime";
// pub const sparkLegacyINT96MetadataKey: &str = "org.apache.spark.legacyINT96";
//
// Spark 3.0.0/3.1.0 是 Go 版用于判断旧历法写入格式的版本边界。
// pub static sparkDatetimeRebaseCutoff: metadata::AppVersion =
//     metadata::NewAppVersionExplicit(sparkAppName, 3, 0, 0);
// pub static sparkINT96RebaseCutoff: metadata::AppVersion =
//     metadata::NewAppVersionExplicit(sparkAppName, 3, 1, 0);
//
// pub const microsPerDay: i64 = 24 * time::Hour / time::Microsecond;
// pub const unixSecondsPerDay: i64 = 24 * time::Hour / time::Second;
// pub const julianDayOfUnixEpoch: i64 = 2440588;
// pub const julianDayNumberConversionOffset: i64 = 32082;
// pub const julianDaysPerFourYearCycle: i64 = 1461;
// pub const julianMonthConversionFactor: i64 = 153;
// pub const julianCalendarYearOffset: i64 = 4800;
//
// sparkVersionFromMetadata 对应 Go 的 metadata 解析入口。
// 它优先读取 Spark 专用 key，缺失时退回 CreatedBy 字段。
// pub fn sparkVersionFromMetadata(fileMeta: Option<&metadata::FileMetaData>) -> Option<metadata::AppVersion> {
//     let fileMeta = fileMeta?;
//     if let Some(kv) = fileMeta.KeyValueMetadata() {
//         if let Some(version) = kv.FindValue(sparkVersionMetadataKey) {
//             return sparkAppVersion(version.to_string());
//         }
//     }
//
//     metadata::NewAppVersion(fileMeta.GetCreatedBy())
// }
//
// sparkAppVersion 对应 Go 的字符串规范化：只有解析结果 app 名为 spark 才返回版本。
// pub fn sparkAppVersion(version: String) -> Option<metadata::AppVersion> {
//     let version = strings::TrimSpace(&version);
//     if version.is_empty() {
//         return None;
//     }
//
//     let appVersion = metadata::NewAppVersion(format!("{} version {}", sparkAppName, version));
//     if appVersion.App != sparkAppName {
//         return None;
//     }
//     Some(appVersion)
// }
//
// sparkRebaseTimeZoneID 对应 Go 的 footer metadata 判定函数。
// 空字符串表示不走 Spark legacy hybrid-calendar 修正。
// pub fn sparkRebaseTimeZoneID(
//     fileMeta: Option<&metadata::FileMetaData>,
//     cutoff: &metadata::AppVersion,
//     legacyKey: &str,
//     fallback: Option<&time::Location>,
// ) -> String {
//     let Some(fileMeta) = fileMeta else {
//         return String::new();
//     };
//
//     let kv = fileMeta.KeyValueMetadata();
//     let mut legacy = false;
//     if let Some(kv) = kv {
//         if kv.FindValue(legacyKey).is_some() {
//             legacy = true;
//         }
//     }
//
//     if !legacy {
//         if let Some(version) = sparkVersionFromMetadata(Some(fileMeta)) {
//             if version.App == sparkAppName && version.LessThan(cutoff) {
//                 legacy = true;
//             }
//         }
//     }
//
//     if !legacy {
//         return String::new();
//     }
//
//     if let Some(kv) = kv {
//         if let Some(tzName) = kv.FindValue(sparkTimeZoneMetadataKey) {
//             let timeZoneID = strings::TrimSpace(tzName);
//             if sparkJulianGregorianRebaseMicrosIndex(timeZoneID).is_some() {
//                 return timeZoneID.to_string();
//             }
// Go 版有意不模拟 Java TimeZone alias 和历史 tzdb 细节；
// 只接受生成表中存在的时区，保证 rebasing 结果确定。
//         }
//     }
//
//     if let Some(fallback) = fallback {
//         let timeZoneID = fallback.String();
//         if sparkJulianGregorianRebaseMicrosIndex(&timeZoneID).is_some() {
//             return timeZoneID;
//         }
//     }
//
// Spark footer 的时区可选；UTC 是最后兜底，避免额外本地时间偏移。
//     sparkRebaseDefaultTimeZoneID.to_string()
// }
//
// julianDayNumberToDate 对应 Go 的 Julian day number 转 Julian calendar 日期。
// 返回 (year, month, day)，用于早期 DATE/TIMESTAMP fallback 分支。
// pub fn julianDayNumberToDate(jdn: i64) -> (i32, time::Month, i32) {
//     let c = jdn + julianDayNumberConversionOffset;
//     let d = (4 * c + 3) / julianDaysPerFourYearCycle;
//     let e = c - (julianDaysPerFourYearCycle * d) / 4;
//     let m = (5 * e + 2) / julianMonthConversionFactor;
//     let day = (e - (julianMonthConversionFactor * m + 2) / 5 + 1) as i32;
//     let month = time::Month(m + 3 - 12 * (m / 10));
//     let year = (d - julianCalendarYearOffset + m / 10) as i32;
//     (year, month, day)
// }
//
// rebaseJulianToGregorianDays 对应 Go 的 DATE rebasing。
// 生成表覆盖范围前的极早日期，先转 Julian 日期标签再按 Gregorian 轴重编码。
// pub fn rebaseJulianToGregorianDays(days: i32) -> i32 {
//     if days < sparkLegacyDateRebaseSwitchDays[0] {
//         let (year, month, day) = julianDayNumberToDate(days as i64 + julianDayOfUnixEpoch);
//         return (time::Date(year, month, day, 0, 0, 0, 0, time::UTC).Unix() / unixSecondsPerDay) as i32;
//     }
//
//     let mut i = sparkLegacyDateRebaseSwitchDays.len();
//     while i > 1 && days < sparkLegacyDateRebaseSwitchDays[i - 1] {
//         i -= 1;
//     }
//     days + sparkLegacyDateRebaseDiffs[i - 1]
// }
//
// sparkRebaseMicrosLookup 对应 Go 的查表结构，缓存某个时区的 switches/diffs 切片。
// pub struct sparkRebaseMicrosLookup {
//     pub timeZoneID: String,
//     pub switches: &'static [i64],
//     pub diffs: &'static [i64],
// }
//
// Spark legacy timestamp rebasing 只应用于 1900-01-01T00:00:00Z 之前的 instant。
// pub static legacyTimestampRebaseCutoffMicros: i64 =
//     time::Date(1900, time::January, 1, 0, 0, 0, 0, time::UTC).UnixMicro();
//
// sparkRebaseMicrosFromMetadata 对应 Go 的 metadata 到 lookup 构造流程。
// pub fn sparkRebaseMicrosFromMetadata(
//     fileMeta: Option<&metadata::FileMetaData>,
//     cutoff: &metadata::AppVersion,
//     legacyKey: &str,
//     fallback: Option<&time::Location>,
// ) -> Result<sparkRebaseMicrosLookup, Error> {
//     let timeZoneID = sparkRebaseTimeZoneID(fileMeta, cutoff, legacyKey, fallback);
//     if timeZoneID.is_empty() {
//         return Ok(sparkRebaseMicrosLookup { timeZoneID, switches: &[], diffs: &[] });
//     }
//     newSparkRebaseMicrosLookup(timeZoneID)
// }
//
// newSparkRebaseMicrosLookup 对应 Go 的时区索引查找。
// 理论上生产路径只会传入生成表已知时区；错误分支保留表损坏/调用方误用诊断。
// pub fn newSparkRebaseMicrosLookup(timeZoneID: String) -> Result<sparkRebaseMicrosLookup, Error> {
//     let Some(index) = sparkJulianGregorianRebaseMicrosIndex(&timeZoneID) else {
//         return Err(Error::new(format!("unknown Spark legacy timestamp rebase timezone {:?}", timeZoneID)));
//     };
//
//     let (switches, diffs) = sparkJulianGregorianRebaseMicrosSlices(index);
//     if switches.is_empty() {
//         return Err(Error::new(format!("empty Spark legacy timestamp rebase table for timezone {:?}", timeZoneID)));
//     }
//     Ok(sparkRebaseMicrosLookup { timeZoneID, switches, diffs })
// }
//
// impl sparkRebaseMicrosLookup {
// rebase 对应 Go 的方法：1900 之后直接返回；早于首个 switch 时走 Spark fallback。
//     pub fn rebase(&self, micros: i64) -> Result<i64, Error> {
//         if micros >= legacyTimestampRebaseCutoffMicros {
//             return Ok(micros);
//         }
//         if self.switches.is_empty() {
//             return Err(Error::new(format!("empty Spark legacy timestamp rebase table for timezone {:?}", self.timeZoneID)));
//         }
//         if micros < self.switches[0] {
//             return Ok(rebaseSparkJulianToGregorianMicrosBeforeSwitch(micros, self.switches[0], self.diffs[0]));
//         }
//
//         let mut i = self.switches.len();
//         while i > 1 && micros < self.switches[i - 1] {
//             i -= 1;
//         }
//         Ok(micros + self.diffs[i - 1])
//     }
// }
//
// rebaseSparkJulianToGregorianMicros 对应 Go 的便捷函数：按时区构造 lookup 后执行 rebasing。
// pub fn rebaseSparkJulianToGregorianMicros(timeZoneID: String, micros: i64) -> Result<i64, Error> {
//     if micros >= legacyTimestampRebaseCutoffMicros {
//         return Ok(micros);
//     }
//     let lookup = newSparkRebaseMicrosLookup(timeZoneID)?;
//     lookup.rebase(micros)
// }
//
// floorDivInt64 对应 Go 辅助函数，保留 Java/Scala floor division 语义。
// Go 的整数除法向零截断，负数时间戳拆分 day/micros-of-day 时不能直接使用。
// pub fn floorDivInt64(x: i64, y: i64) -> i64 {
//     let mut q = x / y;
//     let r = x % y;
//     if r != 0 && (r < 0) != (y < 0) {
//         q -= 1;
//     }
//     q
// }
//
// floorModInt64 对应 Go 辅助函数，匹配 Java/Scala Math.floorMod。
// pub fn floorModInt64(x: i64, y: i64) -> i64 {
//     x - floorDivInt64(x, y) * y
// }
//
// rebaseSparkJulianToGregorianMicrosBeforeSwitch 对应 Spark 生成表起点前的 fallback 分支。
// 它保留本地时间标签：先加源侧 offset 得到 Julian 本地日期，再编码到 Gregorian 轴并减去目标 offset。
// pub fn rebaseSparkJulianToGregorianMicrosBeforeSwitch(
//     micros: i64,
//     firstSwitch: i64,
//     firstDiff: i64,
// ) -> i64 {
//     let julianCommonEraStartMicros = sparkLegacyDateRebaseSwitchDays[0] as i64 * microsPerDay;
//     let gregorianCommonEraStartMicros = time::Date(1, time::January, 1, 0, 0, 0, 0, time::UTC).UnixMicro();
//     let sourceOffset = julianCommonEraStartMicros - firstSwitch;
//     let targetOffset = gregorianCommonEraStartMicros - firstSwitch - firstDiff;
//
//     let localMicros = micros + sourceOffset;
//     let localDays = floorDivInt64(localMicros, microsPerDay);
//     let microsOfDay = floorModInt64(localMicros, microsPerDay);
//     let (year, month, day) = julianDayNumberToDate(localDays + julianDayOfUnixEpoch);
//     let gregorianLocalMicros =
//         time::Date(year, month, day, 0, 0, 0, 0, time::UTC).UnixMicro() + microsOfDay;
//     gregorianLocalMicros - targetOffset
// }
// */
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::sync::OnceLock;
/// Spark DATE rebase 各区间的日偏移（与 switch 表平行）。
const sparkLegacyDateRebaseDiffs: [i32; 14] = [2, 1, 0, -1, -2, -3, -4, -5, -6, -7, -8, -9, -10, 0];
/// Spark DATE rebase 区间起点（相对 Unix epoch 的天数）。
const sparkLegacyDateRebaseSwitchDays: [i32; 14] = [
    -719164, -682945, -646420, -609895, -536845, -500320, -463795, -390745, -354220, -317695,
    -244645, -208120, -171595, -141427,
];
/// 从 `.go` 生成源懒解析出的微秒 switches/diffs 与时区索引。
struct GeneratedRebase {
    switches: Vec<i64>,
    diffs: Vec<i64>,
    zones: BTreeMap<String, (usize, usize)>,
}
/// 解析 `spark_rebase_micros_generated.go` 中的数组与记录。
fn parse_generated_rebase() -> GeneratedRebase {
    let source = include_str!("spark_rebase_micros_generated.go");
    // 从 Go 数组字面量中抽取整数，忽略行尾注释。
    fn numbers(source: &str, start: &str) -> Vec<i64> {
        let body = source
            .split_once(start)
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        body.lines()
            .map(|line| line.split_once("//").map_or(line, |v| v.0))
            .flat_map(|line| line.split(|c: char| !(c.is_ascii_digit() || c == '-')))
            .filter(|v| !v.is_empty() && *v != "-")
            .map(|v| v.parse().unwrap())
            .collect()
    }
    let switches = numbers(
        source,
        "var sparkJulianGregorianRebaseMicrosSwitches = [...]int64{",
    );
    let diffs = numbers(
        source,
        "var sparkJulianGregorianRebaseMicrosDiffs = [...]int64{",
    );
    let records = source
        .split_once("var sparkJulianGregorianRebaseMicrosRecords")
        .unwrap()
        .1;
    let mut zones = BTreeMap::new();
    for line in records.lines() {
        let Some(zone_start) = line.find("timeZoneID: \"") else {
            continue;
        };
        let tail = &line[zone_start + 13..];
        let Some(zone_end) = tail.find('"') else {
            continue;
        };
        let zone = &tail[..zone_end];
        let Some(offset_start) = line.find("offset: ") else {
            continue;
        };
        let offset_tail = &line[offset_start + 8..];
        let offset: usize = offset_tail
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let Some(length_start) = line.find("length: ") else {
            continue;
        };
        let length_tail = &line[length_start + 8..];
        let length: usize = length_tail
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap();
        zones.insert(zone.into(), (offset, length));
    }
    GeneratedRebase {
        switches,
        diffs,
        zones,
    }
}
/// 进程内单次解析缓存。
fn generated() -> &'static GeneratedRebase {
    static DATA: OnceLock<GeneratedRebase> = OnceLock::new();
    DATA.get_or_init(parse_generated_rebase)
}
/// 时区 ID → (offset, length)。
fn rebase_index(zone: &str) -> Option<(usize, usize)> {
    generated().zones.get(zone).copied()
}
/// 按索引切出 switches 与 diffs 平行切片。
fn rebase_slices(index: (usize, usize)) -> (&'static [i64], &'static [i64]) {
    let (offset, length) = index;
    (
        &generated().switches[offset..offset + length],
        &generated().diffs[offset..offset + length],
    )
}
/// footer 未给出合法时区时的兜底 ID。
pub const SPARK_REBASE_DEFAULT_TIME_ZONE_ID: &str = "UTC";
/// 一天的微秒数。
pub const MICROS_PER_DAY: i64 = 86_400_000_000;
/// Spark legacy TIMESTAMP rebasing 的上界（1900-01-01T00:00:00Z）。
const LEGACY_TIMESTAMP_REBASE_CUTOFF_MICROS: i64 = -2_208_988_800_000_000;
/// Unix epoch（1970-01-01）对应的儒略日。
const JULIAN_DAY_OF_UNIX_EPOCH: i64 = 2_440_588;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Spark 应用版本，用于与 rebase cutoff 比较。
pub struct AppVersion {
    pub app: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}
impl AppVersion {
    /// 解析 `spark version X.Y.Z` 或裸版本串。
    pub fn parse_spark(input: &str) -> Option<Self> {
        let text = input.trim();
        let version = text.strip_prefix("spark version ").unwrap_or(text);
        let mut p = version.split('.');
        Some(Self {
            app: "spark".into(),
            major: p.next()?.parse().ok()?,
            minor: p.next().unwrap_or("0").parse().ok()?,
            patch: p
                .next()
                .unwrap_or("0")
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()?,
        })
    }
    /// 同 app 名下的版本三元组比较。
    pub fn less_than(&self, other: &Self) -> bool {
        self.app == other.app
            && (self.major, self.minor, self.patch) < (other.major, other.minor, other.patch)
    }
}
#[derive(Clone, Debug, Default)]
/// footer 中与 Spark rebase 相关的元数据子集。
pub struct SparkFileMeta {
    pub created_by: String,
    pub key_values: BTreeMap<String, String>,
}
/// 优先读专用 key，否则从 CreatedBy 解析。
pub fn spark_version_from_metadata(meta: &SparkFileMeta) -> Option<AppVersion> {
    if let Some(version) = meta.key_values.get("org.apache.spark.version") {
        return AppVersion::parse_spark(version);
    }
    let lower = meta.created_by.to_ascii_lowercase();
    let start = lower.find("spark version ")?;
    AppVersion::parse_spark(&lower[start..])
}
/// 判定是否 legacy 并返回用于查表的时区 ID；非 legacy 返回空串。
pub fn spark_rebase_time_zone_id(
    meta: &SparkFileMeta,
    cutoff: &AppVersion,
    legacy_key: &str,
    fallback: &str,
) -> String {
    // metadata 显式 legacy key，或 Spark 版本低于 cutoff，均视为旧历法写入。
    let legacy = meta.key_values.contains_key(legacy_key)
        || spark_version_from_metadata(meta).is_some_and(|v| v.less_than(cutoff));
    if !legacy {
        return String::new();
    }
    if let Some(zone) = meta
        .key_values
        .get("org.apache.spark.timeZone")
        .map(|v| v.trim())
        .filter(|v| rebase_index(v).is_some())
    {
        return zone.into();
    }
    if rebase_index(fallback).is_some() {
        return fallback.into();
    }
    SPARK_REBASE_DEFAULT_TIME_ZONE_ID.into()
}
/// 儒略日号转公历年月日（Spark/Arrow 同源算法）。
pub fn julian_day_number_to_date(jdn: i64) -> (i32, u32, u32) {
    let c = jdn + 32082;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    (
        (d - 4800 + m / 10) as i32,
        (m + 3 - 12 * (m / 10)) as u32,
        (e - (153 * m + 2) / 5 + 1) as u32,
    )
}
/// 公历日期到 Unix epoch 日数。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    era * 146097 + (yoe * 365 + yoe / 4 - yoe / 100 + doy) - 719468
}
/// DATE：按 switch 表加 diff；早于首个 switch 则走完整儒略→公历换算。
pub fn rebase_julian_to_gregorian_days(days: i32) -> i32 {
    if days < sparkLegacyDateRebaseSwitchDays[0] {
        let (y, m, d) = julian_day_number_to_date(days as i64 + JULIAN_DAY_OF_UNIX_EPOCH);
        return days_from_civil(y as i64, m as i64, d as i64) as i32;
    }
    let mut i = sparkLegacyDateRebaseSwitchDays.len();
    while i > 1 && days < sparkLegacyDateRebaseSwitchDays[i - 1] {
        i -= 1;
    }
    days + sparkLegacyDateRebaseDiffs[i - 1]
}
#[derive(Clone, Debug)]
/// 某一时区的 TIMESTAMP 微秒 rebase 查找表。
pub struct SparkRebaseMicrosLookup {
    pub time_zone_id: String,
    switches: &'static [i64],
    diffs: &'static [i64],
}
impl SparkRebaseMicrosLookup {
    /// 按时区加载 switches/diffs；未知时区报错。
    pub fn new(zone: &str) -> Result<Self> {
        let index = rebase_index(zone).ok_or_else(|| {
            Error(format!(
                "unknown Spark legacy timestamp rebase timezone {zone:?}"
            ))
        })?;
        let (switches, diffs) = rebase_slices(index);
        if switches.is_empty() || switches.len() != diffs.len() {
            return Err(Error(format!(
                "empty Spark legacy timestamp rebase table for timezone {zone:?}"
            )));
        }
        Ok(Self {
            time_zone_id: zone.into(),
            switches,
            diffs,
        })
    }
    /// 对 Unix 微秒做 Julian→Gregorian 修正；近代时间（≥ 1900 附近）直接返回。
    pub fn rebase(&self, micros: i64) -> Result<i64> {
        // 约 1900-01-01 之后无需 rebase（与 Spark 阈值对齐）。
        if micros >= LEGACY_TIMESTAMP_REBASE_CUTOFF_MICROS {
            return Ok(micros);
        }
        if micros < self.switches[0] {
            return Ok(rebase_before_switch(
                micros,
                self.switches[0],
                self.diffs[0],
            ));
        }
        let mut i = self.switches.len();
        while i > 1 && micros < self.switches[i - 1] {
            i -= 1;
        }
        micros
            .checked_add(self.diffs[i - 1])
            .ok_or_else(|| Error("rebased timestamp overflow".into()))
    }
}
/// 向负无穷取整的除法，支持正负除数。
pub fn floor_div_i64(x: i64, y: i64) -> i64 {
    let mut quotient = x / y;
    let remainder = x % y;
    if remainder != 0 && (remainder < 0) != (y < 0) {
        quotient -= 1;
    }
    quotient
}
/// 与 floor_div 配套的余数；符号随除数，与 Go/Spark 实现一致。
pub fn floor_mod_i64(x: i64, y: i64) -> i64 {
    x.wrapping_sub(floor_div_i64(x, y).wrapping_mul(y))
}
/// 早于首个 switch 的微秒：经儒略日完整换算到公历。
pub fn rebase_before_switch(micros: i64, first_switch: i64, first_diff: i64) -> i64 {
    let julian_ce = sparkLegacyDateRebaseSwitchDays[0] as i64 * MICROS_PER_DAY;
    let gregorian_ce = days_from_civil(1, 1, 1) * MICROS_PER_DAY;
    let source_offset = julian_ce - first_switch;
    let target_offset = gregorian_ce - first_switch - first_diff;
    let local = micros + source_offset;
    let days = floor_div_i64(local, MICROS_PER_DAY);
    let tod = floor_mod_i64(local, MICROS_PER_DAY);
    let (y, m, d) = julian_day_number_to_date(days + JULIAN_DAY_OF_UNIX_EPOCH);
    days_from_civil(y as i64, m as i64, d as i64) * MICROS_PER_DAY + tod - target_offset
}
/// 构造 lookup 并 rebase 一次。
pub fn rebase_spark_julian_to_gregorian_micros(zone: &str, micros: i64) -> Result<i64> {
    // Go 在构造 lookup 前执行 cutoff 快速路径；现代时间不依赖 legacy 时区表。
    if micros >= LEGACY_TIMESTAMP_REBASE_CUTOFF_MICROS {
        return Ok(micros);
    }
    SparkRebaseMicrosLookup::new(zone)?.rebase(micros)
}
/// Go 风格别名。
pub fn rebaseJulianToGregorianDays(d: i32) -> i32 {
    rebase_julian_to_gregorian_days(d)
}
/// Go 风格别名。
pub fn floorDivInt64(x: i64, y: i64) -> i64 {
    floor_div_i64(x, y)
}
/// Go 风格别名。
pub fn floorModInt64(x: i64, y: i64) -> i64 {
    floor_mod_i64(x, y)
}
/// Go 风格别名。
pub fn rebaseSparkJulianToGregorianMicros(z: &str, m: i64) -> Result<i64> {
    rebase_spark_julian_to_gregorian_micros(z, m)
}

// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 时区定位与解析工具（对应 Go `util/timeutil` 中 Location / LoadLocation 等）。
//
// 提供 IANA 命名时区、固定偏移时区与 `SYSTEM`/`Local` 占位；解析 MySQL 风格
// `+/-HH:MM` 偏移，并缓存已加载的 Location，供会话时区与时间运算使用。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock, RwLock};

use chrono::{
    DateTime, FixedOffset, Local, LocalResult, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

use crate::errors::{ErrUnknownTimeZone, TimeUtilError};

#[derive(Debug, Clone, PartialEq, Eq)]
/// 时区位置：命名 IANA、固定偏移或进程本地（Local/System）。
pub enum Location {
    Named(Tz),
    Fixed { name: String, offset: FixedOffset },
    Local,
}

impl Location {
    /// 构造 UTC 命名时区。
    pub fn utc() -> Self {
        Self::Named(chrono_tz::UTC)
    }

    /// 按东向 UTC 秒数构造固定偏移时区；偏移非法时返回错误。
    pub fn fixed(name: impl Into<String>, seconds_east_of_utc: i32) -> Result<Self, TimeUtilError> {
        let offset =
            FixedOffset::east_opt(seconds_east_of_utc).ok_or(TimeUtilError::InvalidOffset {
                seconds: seconds_east_of_utc,
            })?;
        Ok(Self::Fixed {
            name: name.into(),
            offset,
        })
    }

    /// 返回时区显示名（命名时区用 IANA 名，Local 为 `"Local"`）。
    pub fn String(&self) -> String {
        match self {
            Self::Named(timezone) => timezone.name().to_owned(),
            Self::Fixed { name, .. } => name.clone(),
            Self::Local => "Local".to_owned(),
        }
    }

    /// 计算给定 UTC 时刻相对该时区的本地偏移（秒）。
    pub fn offset_at_utc(&self, instant: DateTime<Utc>) -> i32 {
        match self {
            Self::Named(timezone) => timezone
                .offset_from_utc_datetime(&instant.naive_utc())
                .fix()
                .local_minus_utc(),
            Self::Fixed { offset, .. } => offset.local_minus_utc(),
            Self::Local => Local
                .offset_from_utc_datetime(&instant.naive_utc())
                .fix()
                .local_minus_utc(),
        }
    }

    /// 将无时区本地墙钟时间转换到 UTC；歧义时刻取较早一次（与 Go 一致）。
    pub fn local_datetime_to_utc(
        &self,
        local: NaiveDateTime,
    ) -> Result<DateTime<Utc>, TimeUtilError> {
        match self {
            Self::Named(timezone) => resolve_local(timezone.from_local_datetime(&local)),
            Self::Fixed { offset, .. } => resolve_local(offset.from_local_datetime(&local)),
            Self::Local => resolve_local(Local.from_local_datetime(&local)),
        }
    }
}

// 处理 chrono LocalResult：Single / Ambiguous / None。
/// 解析本地时间到 UTC，歧义时取 earlier。
fn resolve_local<TzType: TimeZone>(
    result: LocalResult<DateTime<TzType>>,
) -> Result<DateTime<Utc>, TimeUtilError> {
    match result {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        // Go resolves a repeated wall-clock time to one valid occurrence. Keep
        // that behavior deterministic by selecting the earlier occurrence.
        LocalResult::Ambiguous(earlier, _) => Ok(earlier.with_timezone(&Utc)),
        LocalResult::None => Err(TimeUtilError::InvalidLocalTime),
    }
}

/// 进程级 Location 名称缓存。
fn location_cache() -> &'static RwLock<HashMap<String, Location>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Location>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 与 Go `System` 占位对应的系统时区名存储。
fn system_timezone() -> &'static RwLock<String> {
    static SYSTEM_TIMEZONE: OnceLock<RwLock<String>> = OnceLock::new();
    SYSTEM_TIMEZONE.get_or_init(|| RwLock::new("System".to_owned()))
}

// 保证 SetSystemTZ 仅生效一次，对齐 Go sync.Once。
static SET_SYSTEM_TIMEZONE_ONCE: Once = Once::new();

/// Initializes the cache and the Go-compatible `System` placeholder.
/// 初始化 Location 缓存与 Go 兼容的 `System` 占位符。
pub fn init() {
    let _ = location_cache();
    let _ = system_timezone();
}

/// Reads one symbolic-link step without recursively resolving the target.
/// 只读一层符号链接，不递归解析最终目标（对齐 Go InferOneStepLinkForPath）。
pub fn infer_one_step_link_for_path(path: impl AsRef<Path>) -> Result<PathBuf, TimeUtilError> {
    let path = path.as_ref();
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| TimeUtilError::io("lstat", path, error))?;
    if metadata.file_type().is_symlink() {
        return std::fs::read_link(path)
            .map_err(|error| TimeUtilError::io("readlink", path, error));
    }
    Ok(path.to_path_buf())
}

/// Reads the system timezone from `TZ`, then `/etc/localtime`, and falls back
/// 从环境变量 `TZ`、`/etc/localtime` 推断系统时区，失败则回退 UTC（与 Go 一致）。
/// to UTC exactly as the Go implementation does.
pub fn InferSystemTZ() -> String {
    match std::env::var_os("TZ") {
        Some(value) => {
            let timezone = value.to_string_lossy();
            if !timezone.is_empty() && timezone != "UTC" && load_named_location(&timezone).is_ok() {
                return timezone.into_owned();
            }
        }
        None => {
            // 处理 posixrules 等特殊链接，改用单步链接路径再推 IANA 名。
            if let Ok(mut path) = std::fs::canonicalize("/etc/localtime") {
                if path.to_string_lossy().contains("posixrules") {
                    match infer_one_step_link_for_path("/etc/localtime") {
                        Ok(one_step) => path = one_step,
                        Err(_) => return String::new(),
                    }
                }
                if let Some(path) = path.to_str()
                    && let Ok(name) = infer_tz_name_from_file_name(path)
                {
                    return name;
                }
            }
        }
    }
    "UTC".to_owned()
}

/// Gets an IANA timezone name from a zoneinfo path.
/// 从 zoneinfo 文件路径截取 IANA 时区名。
pub fn infer_tz_name_from_file_name(path: &str) -> Result<String, TimeUtilError> {
    for marker in ["zoneinfo.default", "zoneinfo"] {
        if let Some(index) = path.find(marker) {
            let start = index + marker.len() + 1;
            if start <= path.len() {
                return Ok(path[start..].to_owned());
            }
        }
    }
    Err(TimeUtilError::UnsupportedZoneInfoPath {
        path: path.to_owned(),
    })
}

/// 返回当前系统 Location；名无效时退回 Local。
pub fn SystemLocation() -> Location {
    let name = system_timezone()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    LoadLocation(&name).unwrap_or(Location::Local)
}

/// Sets the system timezone once, matching `sync.Once` in Go.
/// 仅设置一次系统时区名，语义对齐 Go `sync.Once`。
pub fn SetSystemTZ(name: &str) {
    SET_SYSTEM_TIMEZONE_ONCE.call_once(|| {
        *system_timezone()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = name.to_owned();
    });
}

/// 读取已设置的系统时区名；仍为占位 `System` 或空则报错。
pub fn GetSystemTZ() -> Result<String, TimeUtilError> {
    let name = system_timezone()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if name.is_empty() || name == "System" {
        return Err(TimeUtilError::InvalidSystemTimeZone);
    }
    Ok(name)
}

/// 按 IANA 名解析为 Named Location。
fn load_named_location(name: &str) -> Result<Location, TimeUtilError> {
    // Go time.LoadLocation recognizes "Local" as the process-local location
    // in addition to IANA database names.
    if name == "Local" {
        return Ok(Location::Local);
    }
    name.parse::<Tz>()
        .map(Location::Named)
        .map_err(|_| TimeUtilError::InvalidTimeZoneName {
            name: name.to_owned(),
        })
}

/// Loads and caches an IANA timezone location.
/// 加载并缓存 IANA 时区；`System` 映射为 Local。
pub fn LoadLocation(name: &str) -> Result<Location, TimeUtilError> {
    if name == "System" {
        return Ok(Location::Local);
    }

    if let Some(location) = location_cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(name)
        .cloned()
    {
        return Ok(location);
    }

    let location = load_named_location(name)?;
    location_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(name.to_owned(), location.clone());
    Ok(location)
}

/// Returns the current timezone name and UTC offset in seconds.
/// 返回当前时区名与相对 UTC 的偏移秒数；Local 对外显示为 `System`。
pub fn Zone(location: &Location) -> (String, i64) {
    let mut name = location.String();
    if name == "Local" {
        name = "System".to_owned();
    }
    (name, i64::from(location.offset_at_utc(Utc::now())))
}

/// Returns a timezone name, formatting unnamed fixed offsets as `+/-HH:MM`.
/// 返回可读时区名；无名固定偏移格式化为 `+/-HH:MM`。
pub fn ZoneName(location: &Location) -> String {
    let (name, offset) = Zone(location);
    if !name.is_empty() {
        return name;
    }

    let sign = if offset < 0 { '-' } else { '+' };
    let absolute = offset.abs();
    format!("{sign}{:02}:{:02}", absolute / 3600, absolute % 3600 / 60)
}

/// Constructs a named timezone, or an unnamed fixed timezone when name is empty.
/// 名非空则 LoadLocation；名为空则按 offset 构造固定时区。
pub fn ConstructTimeZone(name: &str, offset: i32) -> Result<Location, TimeUtilError> {
    if !name.is_empty() {
        return LoadLocation(name);
    }
    Location::fixed("", offset)
}

/// Tests whether `now` is in the inclusive daily interval from `start` to `end`.
/// 判断 `now` 是否落在每日闭区间 [start, end]（可跨午夜）。
pub fn WithinDayTimePeriod<StartZone, EndZone, NowZone>(
    start: DateTime<StartZone>,
    end: DateTime<EndZone>,
    now: DateTime<NowZone>,
) -> bool
where
    StartZone: TimeZone,
    EndZone: TimeZone,
    NowZone: TimeZone,
{
    let start = start.with_timezone(&Utc);
    let end = end.with_timezone(&Utc);
    let now = now.with_timezone(&Utc);
    let start_minute = start.hour() * 60 + start.minute();
    let end_minute = end.hour() * 60 + end.minute();
    let now_minute = now.hour() * 60 + now.minute();

    if end_minute >= start_minute {
        now_minute >= start_minute && now_minute <= end_minute
    } else {
        now_minute <= end_minute || now_minute >= start_minute
    }
}

// 解析 `HH:MM` 或 `HH:MM:SS` 为总秒数。
/// 解析时长字符串为秒；格式非法返回 None。
fn parse_duration_seconds(value: &str) -> Option<i32> {
    let fields: Vec<_> = value.split(':').collect();
    if !(2..=3).contains(&fields.len()) || fields.iter().any(|field| field.is_empty()) {
        return None;
    }
    let hours = fields[0].parse::<i32>().ok()?;
    let minutes = fields[1].parse::<i32>().ok()?;
    let seconds = if fields.len() == 3 {
        fields[2].parse::<i32>().ok()?
    } else {
        0
    };
    if hours < 0 || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return None;
    }
    hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)
}

/// Parses SYSTEM, IANA names, and MySQL `+/-HH:MM` timezone offsets.
/// 解析 SYSTEM、IANA 名或 MySQL `+/-HH:MM` 偏移为 Location。
pub fn ParseTimeZone(value: &str) -> Result<Location, TimeUtilError> {
    if value.eq_ignore_ascii_case("SYSTEM") {
        return Ok(SystemLocation());
    }

    if let Ok(location) = load_named_location(value) {
        return Ok(location);
    }

    // MySQL 允许的偏移范围：负向至 -12:59，正向至 +14:00。
    let bytes = value.as_bytes();
    if let Some(sign @ (b'+' | b'-')) = bytes.first().copied()
        && let Some(seconds) = parse_duration_seconds(&value[1..])
    {
        let within_range = if sign == b'-' {
            seconds <= 12 * 3600 + 59 * 60
        } else {
            seconds <= 14 * 3600
        };
        if within_range {
            let offset = if sign == b'-' { -seconds } else { seconds };
            return Location::fixed("", offset);
        }
    }

    Err(ErrUnknownTimeZone.GenWithStackByArgs(value))
}

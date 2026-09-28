// Copyright 2026 AsterSQL.
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

// timeutil 迁移补充单元测试。
//
// 覆盖可取消 Sleep、时区名/软链推断、系统 TZ 环境变量语义、
// ParseTimeZone 偏移与错误分支、ZoneName 格式化、ConstructTimeZone、
// 日内时间窗以及 Location 缓存行为，对齐 Go 用例。

use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, TimeZone, Utc};
use tempfile::tempdir;

use super::errors::{ErrUnknownTimeZone, TimeUtilError};
use super::time::{CancellationToken, Sleep, SleepError};
use super::time_zone::{
    ConstructTimeZone, GetSystemTZ, InferSystemTZ, LoadLocation, Location, ParseTimeZone,
    SetSystemTZ, WithinDayTimePeriod, Zone, ZoneName, infer_one_step_link_for_path,
    infer_tz_name_from_file_name,
};

/// 串行化对本测试进程内 `TZ` 环境变量的修改，避免并行测试互相干扰。
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 取消令牌先触发时，Sleep 应返回 Cancelled，且耗时远小于原等待。
#[tokio::test]
async fn sleep_returns_context_error_before_the_timer_like_go() {
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
    });

    let started = Instant::now();
    let err = Sleep(&token, Duration::from_secs(10)).await.unwrap_err();
    assert_eq!(err, SleepError::Cancelled);
    assert!(started.elapsed() >= Duration::from_millis(20));
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 计时器先到期时，Sleep 正常完成。
#[tokio::test]
async fn sleep_completes_normally_when_the_timer_wins() {
    let token = CancellationToken::new();
    let started = Instant::now();
    Sleep(&token, Duration::from_millis(5)).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(5));
}

/// 从 zoneinfo 路径推断时区名；非法路径报 unsupported。
#[test]
fn timezone_name_inference_matches_go_path_rules() {
    assert_eq!(
        infer_tz_name_from_file_name("/usr/share/zoneinfo/Asia/Shanghai").unwrap(),
        "Asia/Shanghai"
    );
    assert_eq!(
        infer_tz_name_from_file_name("/usr/share/zoneinfo.default/Asia/Shanghai").unwrap(),
        "Asia/Shanghai"
    );
    assert_eq!(
        infer_tz_name_from_file_name("/tmp/not-zone-data")
            .unwrap_err()
            .to_string(),
        "path /tmp/not-zone-data is not supported"
    );
}

#[cfg(unix)]
#[test]
/// 只解析一层符号链接，不跟完整 canonicalize 链。
fn one_step_link_inference_does_not_resolve_the_whole_chain() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    let file = dir.path().join("real");
    let link_one = dir.path().join("link-one");
    let link_two = dir.path().join("link-two");
    fs::write(&file, b"zoneinfo").unwrap();
    symlink(&file, &link_one).unwrap();
    symlink(&link_one, &link_two).unwrap();

    assert_eq!(infer_one_step_link_for_path(&link_two).unwrap(), link_one);
    assert_eq!(
        fs::canonicalize(link_two).unwrap(),
        fs::canonicalize(file).unwrap()
    );
}

#[test]
/// `InferSystemTZ` 对 TZ 环境变量的处理与 Go 一致（含空串/非法回退 UTC）。
fn infer_system_timezone_honors_go_tz_environment_semantics() {
    let _guard = env_lock().lock().unwrap();
    let previous = std::env::var_os("TZ");

    // SAFETY: all environment mutation in this test binary is serialized by env_lock.
    unsafe { std::env::set_var("TZ", "Asia/Shanghai") };
    assert_eq!(InferSystemTZ(), "Asia/Shanghai");
    // SAFETY: protected by env_lock, as above.
    unsafe { std::env::set_var("TZ", "UTC") };
    assert_eq!(InferSystemTZ(), "UTC");
    // Go time.LoadLocation accepts the special process-local name.
    unsafe { std::env::set_var("TZ", "Local") };
    assert_eq!(InferSystemTZ(), "Local");
    // SAFETY: protected by env_lock, as above.
    unsafe { std::env::set_var("TZ", "") };
    assert_eq!(InferSystemTZ(), "UTC");
    // SAFETY: protected by env_lock, as above.
    unsafe { std::env::set_var("TZ", "not/a-real-zone") };
    assert_eq!(InferSystemTZ(), "UTC");

    match previous {
        Some(value) => {
            // SAFETY: protected by env_lock, as above.
            unsafe { std::env::set_var("TZ", value) };
        }
        None => {
            // SAFETY: protected by env_lock, as above.
            unsafe { std::env::remove_var("TZ") };
        }
    }
}

#[test]
/// ParseTimeZone：SYSTEM/IANA/偏移解析与越界错误类。
fn parse_timezone_matches_system_iana_offset_and_error_branches() {
    SetSystemTZ("Asia/Tokyo");
    assert_eq!(GetSystemTZ().unwrap(), "Asia/Tokyo");

    for (name, expected_offset) in [
        ("SYSTEM", 9 * 3600),
        ("system", 9 * 3600),
        ("Asia/Shanghai", 8 * 3600),
        ("Pacific/Honolulu", -10 * 3600),
        ("-07:00", -7 * 3600),
        ("+02:00", 2 * 3600),
        ("-6:00", -6 * 3600),
    ] {
        let loc = ParseTimeZone(name).unwrap();
        assert_eq!(Zone(&loc).1, expected_offset, "{name}");
    }

    assert!(ParseTimeZone("-12:59").is_ok());
    assert!(ParseTimeZone("+14:00").is_ok());
    for name in ["aa", "-13:00", "+14:01", "+02:60"] {
        let err = ParseTimeZone(name).unwrap_err();
        assert!(ErrUnknownTimeZone.Equal(&err), "{name}: {err}");
        assert!(matches!(err, TimeUtilError::UnknownTimeZone { .. }));
    }
}

#[test]
/// ZoneName：无名偏移格式化为 ±HH:MM，有名则保留原名。
fn zone_name_formats_unnamed_offsets_and_keeps_explicit_names() {
    assert_eq!(ZoneName(&Location::utc()), "UTC");
    assert_eq!(
        ZoneName(&Location::fixed("", 8 * 3600 + 30 * 60).unwrap()),
        "+08:30"
    );
    assert_eq!(
        ZoneName(&Location::fixed("", -(6 * 3600 + 15 * 60)).unwrap()),
        "-06:15"
    );
    assert_eq!(ZoneName(&Location::fixed("", 0).unwrap()), "+00:00");
    assert_eq!(
        ZoneName(&Location::fixed("UTC+8", 8 * 3600).unwrap()),
        "UTC+8"
    );
}

#[test]
/// ConstructTimeZone：有名优先加载 IANA；空名走固定偏移。
fn construct_timezone_prefers_name_and_preserves_fixed_offset_behavior() {
    let local = NaiveDate::from_ymd_opt(2018, 8, 15)
        .unwrap()
        .and_hms_opt(20, 0, 0)
        .unwrap();
    let shanghai = ConstructTimeZone("Asia/Shanghai", -23 * 3600).unwrap();
    assert_eq!(
        shanghai.local_datetime_to_utc(local).unwrap(),
        Utc.with_ymd_and_hms(2018, 8, 15, 12, 0, 0).unwrap()
    );

    let fixed = ConstructTimeZone("", -8 * 3600).unwrap();
    let local = NaiveDate::from_ymd_opt(2018, 8, 15)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    assert_eq!(
        fixed.local_datetime_to_utc(local).unwrap(),
        Utc.with_ymd_and_hms(2018, 8, 15, 20, 0, 0).unwrap()
    );
    assert_eq!(
        ConstructTimeZone("asia/not-exist", 0)
            .unwrap_err()
            .to_string(),
        "invalid name for timezone asia/not-exist"
    );
}

#[test]
/// WithinDayTimePeriod：普通区间与跨午夜区间边界。
fn within_day_period_matches_normal_and_cross_midnight_go_cases() {
    let at = |hour, minute| Utc.with_ymd_and_hms(2026, 7, 14, hour, minute, 59).unwrap();

    assert!(WithinDayTimePeriod(at(0, 0), at(6, 0), at(3, 0)));
    assert!(WithinDayTimePeriod(at(0, 0), at(6, 0), at(6, 0)));
    assert!(!WithinDayTimePeriod(at(0, 0), at(6, 0), at(6, 1)));
    assert!(WithinDayTimePeriod(at(22, 0), at(6, 0), at(23, 0)));
    assert!(WithinDayTimePeriod(at(22, 0), at(6, 0), at(5, 0)));
    assert!(!WithinDayTimePeriod(at(22, 0), at(6, 0), at(12, 0)));
}

#[test]
/// LoadLocation 缓存等价性，以及未知名拒绝。
fn location_cache_returns_equivalent_locations_and_rejects_unknown_names() {
    assert_eq!(LoadLocation("UTC").unwrap(), LoadLocation("UTC").unwrap());
    assert_eq!(
        LoadLocation("not/a-real-zone").unwrap_err().to_string(),
        "invalid name for timezone not/a-real-zone"
    );
}

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

// Copyright 2018 PingCAP, Inc. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// `time_zone` 模块单元测试：系统时区推断、解析、命名与构造。
//
// 覆盖 zoneinfo 路径截取、符号链接单步解析、ParseTimeZone / ZoneName /
// ConstructTimeZone，以及通过子进程注入 `TZ` 的 Local 用例。

use std::process::Command;

use chrono::{NaiveDate, TimeZone, Utc};

use astersql_util_timeutil::errors::ErrUnknownTimeZone;
use astersql_util_timeutil::time_zone::{
    ConstructTimeZone, GetSystemTZ, InferSystemTZ, Location, ParseTimeZone, SetSystemTZ,
    SystemLocation, Zone, ZoneName,
};

#[test]
/// 验证从 zoneinfo / zoneinfo.default 路径截取 IANA 名。
fn test_get_tz_name_from_file_name() {
    assert_eq!(
        astersql_util_timeutil::time_zone::infer_tz_name_from_file_name(
            "/usr/share/zoneinfo/Asia/Shanghai"
        )
        .unwrap(),
        "Asia/Shanghai"
    );
    assert_eq!(
        astersql_util_timeutil::time_zone::infer_tz_name_from_file_name(
            "/usr/share/zoneinfo.default/Asia/Shanghai"
        )
        .unwrap(),
        "Asia/Shanghai"
    );
}

// 子进程用例通过该环境变量带回期望的推断结果。
const LOCAL_CASE_ENV: &str = "TIMEUTIL_TASK_426_LOCAL_CASE";

#[test]
/// 在不同 `TZ` 下子进程验证 InferSystemTZ / SetSystemTZ / SystemLocation。
fn test_local() {
    if let Ok(expected) = std::env::var(LOCAL_CASE_ENV) {
        let inferred = InferSystemTZ();
        assert_eq!(inferred, expected);
        SetSystemTZ(&inferred);
        assert_eq!(GetSystemTZ().unwrap(), expected);
        assert_eq!(SystemLocation().String(), expected);
        return;
    }

    for (tz, expected) in [
        ("Asia/Shanghai", "Asia/Shanghai"),
        ("UTC", "UTC"),
        ("", "UTC"),
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "test_local", "--nocapture"])
            .env("TZ", tz)
            .env(LOCAL_CASE_ENV, expected)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "TZ={tz:?} child failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
/// 验证只解一层符号链接，不递归到最终文件。
fn test_infer_one_step_link_for_path() {
    let directory = tempfile::tempdir().unwrap();
    let link1 = directory.path().join("testlink1");
    let link2 = directory.path().join("testlink2");
    let link3 = directory.path().join("testlink3");
    std::fs::File::create(&link1).unwrap();
    std::os::unix::fs::symlink(&link1, &link2).unwrap();
    std::os::unix::fs::symlink(&link2, &link3).unwrap();

    assert_eq!(
        astersql_util_timeutil::time_zone::infer_one_step_link_for_path(&link3).unwrap(),
        link2
    );
    assert_eq!(
        std::fs::canonicalize(&link3).unwrap(),
        std::fs::canonicalize(&link1).unwrap()
    );
}

#[test]
/// 覆盖 SYSTEM、IANA、偏移与非法名的 ParseTimeZone 行为。
fn test_parse_time_zone() {
    SetSystemTZ("Asia/Tokyo");
    let cases = [
        ("SYSTEM", 9 * 3600, false),
        ("system", 9 * 3600, false),
        ("Asia/Shanghai", 8 * 3600, false),
        ("Pacific/Honolulu", -10 * 3600, false),
        ("-07:00", -7 * 3600, false),
        ("+02:00", 2 * 3600, false),
        ("aa", 0, true),
    ];

    for (name, expected_offset, invalid) in cases {
        match ParseTimeZone(name) {
            Err(error) if invalid => assert!(ErrUnknownTimeZone.Equal(&error), "{name}"),
            Err(error) => panic!("{name}: {error}"),
            Ok(_) if invalid => panic!("{name}: expected unknown timezone"),
            Ok(location) => assert_eq!(Zone(&location).1, expected_offset, "{name}"),
        }
    }
}

#[test]
/// Go `time.LoadLocation("Local")` 返回进程本地时区，ParseTimeZone 也应接受。
fn test_load_and_parse_local_timezone() {
    let loaded = astersql_util_timeutil::time_zone::LoadLocation("Local").unwrap();
    assert_eq!(loaded, Location::Local);

    let parsed = ParseTimeZone("Local").unwrap();
    assert_eq!(parsed, Location::Local);
}

#[test]
/// 验证命名/无名固定偏移的 ZoneName 格式。
fn test_zone_name() {
    let cases = [
        ("iana timezone", Location::utc(), "UTC"),
        (
            "unnamed positive fixed zone",
            Location::fixed("", 8 * 3600 + 30 * 60).unwrap(),
            "+08:30",
        ),
        (
            "unnamed negative fixed zone",
            Location::fixed("", -(6 * 3600 + 15 * 60)).unwrap(),
            "-06:15",
        ),
        (
            "unnamed zero fixed zone",
            Location::fixed("", 0).unwrap(),
            "+00:00",
        ),
        (
            "named fixed zone",
            Location::fixed("UTC+8", 8 * 3600).unwrap(),
            "UTC+8",
        ),
    ];

    for (name, location, expected) in cases {
        assert_eq!(ZoneName(&location), expected, "{name}");
    }
}

/// 断言本地墙钟小时映射到期望的 UTC 小时。
fn assert_same_instant(location: &Location, local_hour: u32, expected_utc_hour: u32) {
    let local = NaiveDate::from_ymd_opt(2018, 8, 15)
        .unwrap()
        .and_hms_opt(local_hour, 0, 0)
        .unwrap();
    let expected = Utc
        .with_ymd_and_hms(2018, 8, 15, expected_utc_hour, 0, 0)
        .unwrap();
    assert_eq!(location.local_datetime_to_utc(local).unwrap(), expected);
}

#[test]
/// 验证空名固定偏移、命名时区忽略 offset、非法名报错。
fn test_construct_time_zone() {
    let location = ConstructTimeZone("", 8 * 3600).unwrap();
    assert_same_instant(&location, 20, 12);

    let location = ConstructTimeZone("", -8 * 3600).unwrap();
    assert_same_instant(&location, 12, 20);

    let location = ConstructTimeZone("", 0).unwrap();
    assert_same_instant(&location, 20, 20);

    for ignored_offset in [23 * 3600, -23 * 3600, 0] {
        let location = ConstructTimeZone("UTC", ignored_offset).unwrap();
        assert_same_instant(&location, 12, 12);
    }

    for ignored_offset in [-23 * 3600, 23 * 3600, 0] {
        let location = ConstructTimeZone("Asia/Shanghai", ignored_offset).unwrap();
        assert_same_instant(&location, 20, 12);
    }

    assert_eq!(
        ConstructTimeZone("asia/not-exist", 0)
            .unwrap_err()
            .to_string(),
        "invalid name for timezone asia/not-exist"
    );
}

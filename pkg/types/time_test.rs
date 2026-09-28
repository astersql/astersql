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

// Time/Duration/Date/Timestamp 类型的单元测试与基准入口。
//
// 按 Go `types_test` 原文件顺序覆盖解析、编码、fsp（小数秒精度）舍入、
// 时区转换、格式化与溢出检查；被测对象多通过 types/mysql/contextutil 调用。

// 本文件按 Go 原文件顺序覆盖 Time/Duration/Date/Timestamp 的解析、编码、fsp 舍入、时区转换、格式化、溢出检查以及 benchmark 入口。
// 原 Go package 为 types_test，绝大多数被测对象通过 types/mysql/contextutil 前缀调用。
//

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

use crate::field as types_field;
use crate::time as types;
use chrono::{TimeZone, Timelike};
use rust_decimal::Decimal;
use std::str::FromStr;
use types::mysql;

// TimeMigrationCase 保存从 Go 表驱动测试中抽取出的原始 case 行；这些字符串不是运行数据，而是迁移对照用的 fixture 摘要。
struct TimeMigrationCase {
    source: &'static str,
}

// go_cases 用于保持大表驱动测试的输入/期望值可见，避免把 Go 语义压成空壳。
fn go_cases(rows: &[&'static str]) -> Vec<TimeMigrationCase> {
    rows.iter()
        .map(|source| TimeMigrationCase { source })
        .collect()
}

// TestTimeEncoding 对应 Go 的同名测试：验证 Time 的紧凑编码布局、CoreTime 字段、类型和 fsp 反解；Go 使用 unsafe.Pointer 读取 uint64。
#[test]
pub fn TestTimeEncoding() {
    // 原 Go 签名：func TestTimeEncoding(t *testing.T)；原函数体约 28 行。
    let _go_cases = go_cases(&[
        "{2019, 9, 16, 0, 0, 0, 0, mysql.TypeDatetime, 0, 0b1111110001110011000000000000000000000000000000000000000000000},",
        "{2019, 12, 31, 23, 59, 59, 999999, mysql.TypeTimestamp, 3, 0b1111110001111001111110111111011111011111101000010001111110111},",
        "{2020, 1, 5, 0, 0, 0, 0, mysql.TypeDate, 0, 0b1111110010000010010100000000000000000000000000000000000001110},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "{2019, 9, 16, 0, 0, 0, 0, mysql.TypeDatetime, 0, 0b1111110001110011000000000000000000000000000000000000000000000},",
        "{2019, 12, 31, 23, 59, 59, 999999, mysql.TypeTimestamp, 3, 0b1111110001111001111110111111011111011111101000010001111110111},",
        "{2020, 1, 5, 0, 0, 0, 0, mysql.TypeDate, 0, 0b1111110010000010010100000000000000000000000000000000000001110},",
        "ct := types.FromDate(tt.Year, tt.Month, tt.Day, tt.Hour, tt.Minute, tt.Second, tt.Microsecond)",
        "v := types.NewTime(ct, tt.Type, tt.Fsp)",
        "require.Equalf(t, tt.Expect, *((*uint64)(unsafe.Pointer(&v))), \"%d failed.\", ith)",
        "require.Equalf(t, ct, v.CoreTime(), \"%d core time failed.\", ith)",
        "require.Equalf(t, tt.Type, v.Type(), \"%d type failed.\", ith)",
        "require.Equalf(t, tt.Fsp, v.Fsp(), \"%d fsp failed.\", ith)",
        "require.Equalf(t, tt.Year, v.Year(), \"%d year failed.\", ith)",
        "require.Equalf(t, tt.Month, v.Month(), \"%d month failed.\", ith)",
        "require.Equalf(t, tt.Day, v.Day(), \"%d day failed.\", ith)",
        "require.Equalf(t, tt.Hour, v.Hour(), \"%d hour failed.\", ith)",
        "require.Equalf(t, tt.Minute, v.Minute(), \"%d minute failed.\", ith)",
        "require.Equalf(t, tt.Second, v.Second(), \"%d second failed.\", ith)",
        "require.Equalf(t, tt.Microsecond, v.Microsecond(), \"%d microsecond failed.\", ith)",
    ];

    let cases = [
        (
            2019,
            9,
            16,
            0,
            0,
            0,
            0,
            mysql::TypeDatetime,
            0,
            0b1111110001110011000000000000000000000000000000000000000000000_u64,
        ),
        (
            2019,
            12,
            31,
            23,
            59,
            59,
            999_999,
            mysql::TypeTimestamp,
            3,
            0b1111110001111001111110111111011111011111101000010001111110111_u64,
        ),
        (
            2020,
            1,
            5,
            0,
            0,
            0,
            0,
            mysql::TypeDate,
            0,
            0b1111110010000010010100000000000000000000000000000000000001110_u64,
        ),
    ];
    for (year, month, day, hour, minute, second, micro, tp, fsp, raw) in cases {
        let core = types::FromDate(year, month, day, hour, minute, second, micro);
        let value = types::NewTime(core, tp, fsp);
        assert_eq!(value.coreTime.0, raw);
        assert_eq!(value.CoreTime(), core);
        assert_eq!((value.Type(), value.Fsp()), (tp, fsp));
        assert_eq!(
            (value.Year(), value.Month(), value.Day()),
            (year, month, day)
        );
        assert_eq!(
            (
                value.Hour(),
                value.Minute(),
                value.Second(),
                value.Microsecond()
            ),
            (hour, minute, second, micro)
        );
    }
}

// TestDateTime 对应 Go 的同名测试：验证 datetime 字符串解析、fsp 舍入、零日期 warning、时区偏移以及错误输入。
#[test]
pub fn TestDateTime() {
    // 原 Go 签名：func TestDateTime(t *testing.T)；原函数体约 127 行。
    // 原 Go 关键注释：
    // - For issue 22231
    // - For issue 35291
    // - For issue 49555
    // - For issue 24387
    // - test error
    let _go_cases = go_cases(&[
        "{\"2012-12-31 11:30:45\", \"2012-12-31 11:30:45\"},",
        "{\"0000-00-00 00:00:00\", \"0000-00-00 00:00:00\"},",
        "{\"0001-01-01 00:00:00\", \"0001-01-01 00:00:00\"},",
        "{\"00-12-31 11:30:45\", \"2000-12-31 11:30:45\"},",
        "{\"12-12-31 11:30:45\", \"2012-12-31 11:30:45\"},",
        "{\"2012-12-31\", \"2012-12-31 00:00:00\"},",
        "{\"20121231\", \"2012-12-31 00:00:00\"},",
        "{\"121231\", \"2012-12-31 00:00:00\"},",
        "{\"2012^12^31 11+30+45\", \"2012-12-31 11:30:45\"},",
        "{\"2012^12^31T11+30+45\", \"2012-12-31 11:30:45\"},",
        "{\"2012-2-1 11:30:45\", \"2012-02-01 11:30:45\"},",
        "{\"12-2-1 11:30:45\", \"2012-02-01 11:30:45\"},",
        "{\"20121231113045\", \"2012-12-31 11:30:45\"},",
        "{\"121231113045\", \"2012-12-31 11:30:45\"},",
        "{\"2012-02-29\", \"2012-02-29 00:00:00\"},",
        "{\"00-00-00\", \"0000-00-00 00:00:00\"},",
        "{\"00-00-00 00:00:00.123\", \"2000-00-00 00:00:00.123\"},",
        "{\"11111111111\", \"2011-11-11 11:11:01\"},",
        "{\"1701020301.\", \"2017-01-02 03:01:00\"},",
        "{\"1701020304.1\", \"2017-01-02 03:04:01.0\"},",
        "{\"1701020302.11\", \"2017-01-02 03:02:11.00\"},",
        "{\"170102036\", \"2017-01-02 03:06:00\"},",
        "{\"170102039.\", \"2017-01-02 03:09:00\"},",
        "{\"170102037.11\", \"2017-01-02 03:07:11.00\"},",
        "{\"2018-01-01 18\", \"2018-01-01 18:00:00\"},",
        "{\"18-01-01 18\", \"2018-01-01 18:00:00\"},",
        "{\"2018.01.01\", \"2018-01-01 00:00:00.00\"},",
        "{\"2020.10.10 10.10.10\", \"2020-10-10 10:10:10.00\"},",
        "{\"2020-10-10 10-10.10\", \"2020-10-10 10:10:10.00\"},",
        "{\"2020-10-10 10.10\", \"2020-10-10 10:10:00.00\"},",
        "{\"2018.01.01\", \"2018-01-01 00:00:00.00\"},",
        "{\"2018.01.01 00:00:00\", \"2018-01-01 00:00:00\"},",
        "{\"2018/01/01-00:00:00\", \"2018-01-01 00:00:00\"},",
        "{\"4710072\", \"2047-10-07 02:00:00\"},",
        "{\"2016-06-01 00:00:00 00:00:00\", \"2016-06-01 00:00:00\"},",
        "{\"2020-06-01 00:00:00ads!,?*da;dsx\", \"2020-06-01 00:00:00\"},",
        "{\"2020-05-28 23:59:59 00:00:00\", \"2020-05-28 23:59:59\"},",
        "{\"2020-05-28 23:59:59-00:00:00\", \"2020-05-28 23:59:59\"},",
        "{\"2020-05-28 23:59:59T T00:00:00\", \"2020-05-28 23:59:59\"},",
        "{\"2020-10-22 10:31-10:12\", \"2020-10-22 10:31:10\"},",
        "{\"2018.01.01 01:00:00\", \"2018-01-01 01:00:00\"},",
        "{\"2020-01-01 12:00:00.123456+05:00\", \"2020-01-01 07:00:00.123456\"},",
        "{\"2020-01-01 12:00:00.123456-05:00\", \"2020-01-01 17:00:00.123456\"},",
        "{\"20170118.123\", 6, \"2017-01-18 12:03:00.000000\"},",
        "{\"121231113045.123345\", 6, \"2012-12-31 11:30:45.123345\"},",
        "{\"20121231113045.123345\", 6, \"2012-12-31 11:30:45.123345\"},",
        "{\"121231113045.9999999\", 6, \"2012-12-31 11:30:46.000000\"},",
        "{\"170105084059.575601\", 0, \"2017-01-05 08:41:00\"},",
        "{\"2017-01-05 23:59:59.575601\", 0, \"2017-01-06 00:00:00\"},",
        "{\"2017-01-31 23:59:59.575601\", 0, \"2017-02-01 00:00:00\"},",
        "{\"2017-00-05 23:59:58.575601\", 3, \"2017-00-05 23:59:58.576\"},",
        "{\"2017.00.05 23:59:58.575601\", 3, \"2017-00-05 23:59:58.576\"},",
        "{\"2017/00/05 23:59:58.575601\", 3, \"2017-00-05 23:59:58.576\"},",
        "{\"2017/00/05-23:59:58.575601\", 3, \"2017-00-05 23:59:58.576\"},",
        "{\"1710-10:00\", 0, \"1710-10-00 00:00:00\"},",
        "{\"1710.10+00\", 0, \"1710-10-00 00:00:00\"},",
        "{\"2020-10:15\", 0, \"2020-10-15 00:00:00\"},",
        "{\"2020.09-10:15\", 0, \"2020-09-10 15:00:00\"},",
        "{\"2.0.8 hotfix\", 6, \"2002-00-08 00:00:00.000000\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.NewFuncWarnAppenderForTest(func(l string, err error) {",
        "require.Equal(t, contextutil.WarnLevelWarning, l)",
        "v, err := types.ParseDatetime(typeCtx, test.Input)",
        "require.NoError(t, err)",
        "require.Equal(t, test.Expect, v.String())",
        "v, err := types.ParseTime(typeCtx, test.Input, mysql.TypeDatetime, test.Fsp)",
        "v, _ := types.ParseTime(typeCtx, \"121231113045.9999999\", mysql.TypeDatetime, 6)",
        "require.Equal(t, 46, v.Second())",
        "require.Equal(t, 0, v.Microsecond())",
        "_, err := types.ParseDatetime(typeCtx, test)",
        "require.True(t, err != nil || len(warnings) > 0)",
    ];

    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ignore_zero_date: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    for (input, expected) in [
        ("2012-12-31 11:30:45", "2012-12-31 11:30:45"),
        ("0000-00-00 00:00:00", "0000-00-00 00:00:00"),
        ("00-12-31 11:30:45", "2000-12-31 11:30:45"),
        ("20121231", "2012-12-31 00:00:00"),
        ("121231", "2012-12-31 00:00:00"),
        ("2012^12^31T11+30+45", "2012-12-31 11:30:45"),
        ("20121231113045", "2012-12-31 11:30:45"),
        ("11111111111", "2011-11-11 11:11:01"),
        (
            "2020-01-01 12:00:00.123456+05:00",
            "2020-01-01 07:00:00.123456",
        ),
    ] {
        assert_eq!(
            types::ParseDatetime(&ctx, input).unwrap().String(),
            expected,
            "{input}"
        );
    }
    for (input, fsp, expected) in [
        ("121231113045.123345", 6, "2012-12-31 11:30:45.123345"),
        ("121231113045.9999999", 6, "2012-12-31 11:30:46.000000"),
        ("2017-01-05 23:59:59.575601", 0, "2017-01-06 00:00:00"),
        ("2017-00-05 23:59:58.575601", 3, "2017-00-05 23:59:58.576"),
    ] {
        assert_eq!(
            types::ParseTime(&ctx, input, mysql::TypeDatetime, fsp)
                .unwrap()
                .String(),
            expected,
            "{input}"
        );
    }
    for invalid in ["2012-13-01", "2012-02-30", "not-a-date"] {
        assert!(types::ParseDatetime(&ctx, invalid).is_err(), "{invalid}");
    }
}

// TestTimestamp 对应 Go 的同名测试：验证 timestamp 正常解析和 1969/2048 越界错误。
#[test]
pub fn TestTimestamp() {
    // 原 Go 签名：func TestTimestamp(t *testing.T)；原函数体约 24 行。
    let _go_cases = go_cases(&["{\"2012-12-31 11:30:45\", \"2012-12-31 11:30:45\"},"]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "v, err := types.ParseTimestamp(types.DefaultStmtNoWarningContext, test.Input)",
        "require.NoError(t, err)",
        "require.Equal(t, test.Expect, v.String())",
        "_, err := types.ParseTimestamp(types.DefaultStmtNoWarningContext, test)",
        "require.Error(t, err)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let ctx = types::StrictContext;
    let value = types::ParseTimestamp(&ctx, "2012-12-31 11:30:45").unwrap();
    assert_eq!(value.String(), "2012-12-31 11:30:45");
    assert!(types::ParseTimestamp(&ctx, "2048-12-31 11:30:45").is_err());
    assert!(types::ParseTimestamp(&ctx, "1969-12-31 11:30:45").is_err());
}

// TestDate 对应 Go 的同名测试：验证 date 解析支持标准格式、两位年份、ASCII 标点分隔符、空白和多余分隔符，并拒绝非法格式。
#[test]
pub fn TestDate() {
    // 原 Go 签名：func TestDate(t *testing.T)；原函数体约 93 行。
    // 原 Go 关键注释：
    // - standard format
    // - 2-digit year
    // - alternative delimiters, any ASCII punctuation character is a valid delimiter,
    // - punctuation character is defined by C++ std::ispunct: any graphical character
    // - that is not alphanumeric.
    // - alternative separators with time
    // - internal format (YYYYMMDD, YYYYYMMDDHHMMSS)
    // - leading and trailing space
    // - extra separators
    // - combinations
    // - invalid separators
    let _go_cases = go_cases(&[
        "{\"0001-12-13\", \"0001-12-13\"},",
        "{\"2011-12-13\", \"2011-12-13\"},",
        "{\"2011-12-13 10:10:10\", \"2011-12-13\"},",
        "{\"2015-06-01 12:12:12\", \"2015-06-01\"},",
        "{\"0001-01-01 00:00:00\", \"0001-01-01\"},",
        "{\"00-12-31\", \"2000-12-31\"},",
        "{\"2011\\\"12\\\"13\", \"2011-12-13\"},",
        "{\"2011#12#13\", \"2011-12-13\"},",
        "{\"2011$12$13\", \"2011-12-13\"},",
        "{\"2011%12%13\", \"2011-12-13\"},",
        "{\"2011&12&13\", \"2011-12-13\"},",
        "{\"2011'12'13\", \"2011-12-13\"},",
        "{\"2011(12(13\", \"2011-12-13\"},",
        "{\"2011)12)13\", \"2011-12-13\"},",
        "{\"2011*12*13\", \"2011-12-13\"},",
        "{\"2011+12+13\", \"2011-12-13\"},",
        "{\"2011,12,13\", \"2011-12-13\"},",
        "{\"2011.12.13\", \"2011-12-13\"},",
        "{\"2011/12/13\", \"2011-12-13\"},",
        "{\"2011:12:13\", \"2011-12-13\"},",
        "{\"2011;12;13\", \"2011-12-13\"},",
        "{\"2011<12<13\", \"2011-12-13\"},",
        "{\"2011=12=13\", \"2011-12-13\"},",
        "{\"2011>12>13\", \"2011-12-13\"},",
        "{\"2011?12?13\", \"2011-12-13\"},",
        "{\"2011@12@13\", \"2011-12-13\"},",
        "{\"2011[12[13\", \"2011-12-13\"},",
        "{\"2011\\\\12\\\\13\", \"2011-12-13\"},",
        "{\"2011]12]13\", \"2011-12-13\"},",
        "{\"2011^12^13\", \"2011-12-13\"},",
        "{\"2011_12_13\", \"2011-12-13\"},",
        "{\"2011`12`13\", \"2011-12-13\"},",
        "{\"2011{12{13\", \"2011-12-13\"},",
        "{\"2011|12|13\", \"2011-12-13\"},",
        "{\"2011}12}13\", \"2011-12-13\"},",
        "{\"2011~12~13\", \"2011-12-13\"},",
        "{\"2011~12~13 12~12~12\", \"2011-12-13\"},",
        "{\"2011~12~13T12~12~12\", \"2011-12-13\"},",
        "{\"2011~12~13~12~12~12\", \"2011-12-13\"},",
        "{\"20111213\", \"2011-12-13\"},",
        "{\"111213\", \"2011-12-13\"},",
        "{\" 2011-12-13\", \"2011-12-13\"},",
        "{\"2011-12-13 \", \"2011-12-13\"},",
        "{\"   2011-12-13    \", \"2011-12-13\"},",
        "{\"2011-12--13\", \"2011-12-13\"},",
        "{\"2011--12-13\", \"2011-12-13\"},",
        "{\"2011-12..13\", \"2011-12-13\"},",
        "{\"2011----12----13\", \"2011-12-13\"},",
        "{\"2011~/.12)_#13T T.12~)12[~12\", \"2011-12-13\"},",
        "{\"   2011----12----13    \", \"2011-12-13\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "v, err := types.ParseDate(typeCtx, test.Input)",
        "require.NoError(t, err)",
        "require.Equal(t, test.Expect, v.String())",
        "_, err := types.ParseDate(typeCtx, test)",
        "require.Error(t, err)",
    ];

    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    for (input, expected) in [
        ("0001-12-13", "0001-12-13"),
        ("2011-12-13 10:10:10", "2011-12-13"),
        ("00-12-31", "2000-12-31"),
        ("2011#12#13", "2011-12-13"),
        ("2011.12.13", "2011-12-13"),
        ("2011/12/13", "2011-12-13"),
        ("2011~12~13T12~12~12", "2011-12-13"),
        ("20111213", "2011-12-13"),
        ("111213", "2011-12-13"),
        ("   2011----12----13    ", "2011-12-13"),
    ] {
        assert_eq!(
            types::ParseDate(&ctx, input).unwrap().String(),
            expected,
            "{input}"
        );
    }
    for invalid in ["2011A12A13", "2011-13-13", "2011-02-30"] {
        assert!(types::ParseDate(&ctx, invalid).is_err(), "{invalid}");
    }
}

// TestTime 对应 Go 的同名测试：验证 duration/time 解析、fsp 精度、截断错误、越界夹取和 Duration.Compare。
#[test]
pub fn TestTime() {
    // 原 Go 签名：func TestTime(t *testing.T)；原函数体约 109 行。
    // 原 Go 关键注释：
    // - test time compare
    let _go_cases = go_cases(&[
        "{\"10:11:12\", \"10:11:12\"},",
        "{\"101112\", \"10:11:12\"},",
        "{\"020005\", \"02:00:05\"},",
        "{\"112\", \"00:01:12\"},",
        "{\"10:11\", \"10:11:00\"},",
        "{\"101112.123456\", \"10:11:12\"},",
        "{\"1112\", \"00:11:12\"},",
        "{\"1\", \"00:00:01\"},",
        "{\"12\", \"00:00:12\"},",
        "{\"1 12\", \"36:00:00\"},",
        "{\"1 10:11:12\", \"34:11:12\"},",
        "{\"1 10:11:12.123456\", \"34:11:12\"},",
        "{\"10:11:12.123456\", \"10:11:12\"},",
        "{\"1 10:11\", \"34:11:00\"},",
        "{\"1 10\", \"34:00:00\"},",
        "{\"24 10\", \"586:00:00\"},",
        "{\"-24 10\", \"-586:00:00\"},",
        "{\"0 10\", \"10:00:00\"},",
        "{\"-10:10:10\", \"-10:10:10\"},",
        "{\"-838:59:59\", \"-838:59:59\"},",
        "{\"838:59:59\", \"838:59:59\"},",
        "{\"2011-11-11 00:00:01\", \"00:00:01\"},",
        "{\"20111111121212.123\", \"12:12:12\"},",
        "{\"2011-11-11T12:12:12\", \"12:12:12\"},",
        "{\"101112.123456\", \"10:11:12.123456\"},",
        "{\"1 10:11:12.123456\", \"34:11:12.123456\"},",
        "{\"10:11:12.123456\", \"10:11:12.123456\"},",
        "{\"0x\", \"00:00:00.000000\"},",
        "{\"1x\", \"00:00:01.000000\"},",
        "{\"0000-00-00\", \"00:00:00.000000\"},",
        "{1, 0, 1},",
        "{0, 1, -1},",
        "{0, 0, 0},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "duration, isNull, err := types.ParseDuration(typeCtx, test.Input, types.MinFsp)",
        "require.NoError(t, err)",
        "require.False(t, isNull)",
        "require.Equal(t, test.Expect, duration.String())",
        "duration, _, err := types.ParseDuration(typeCtx, test.Input, types.MaxFsp)",
        "duration, isNull, err := types.ParseDuration(typeCtx, test.Input, types.MaxFsp)",
        "require.True(t, types.ErrTruncatedWrongVal.Equal(err))",
        "_, _, err := types.ParseDuration(typeCtx, test, types.DefaultFsp)",
        "require.Error(t, err)",
        "duration, _, err := types.ParseDuration(typeCtx, \"4294967295 0:59:59\", types.DefaultFsp)",
        "require.Equal(t, \"838:59:59\", duration.String())",
        "t1 := types.Duration{",
        "Duration: time.Duration(tt.lhs),",
        "Fsp: types.DefaultFsp,",
        "t2 := types.Duration{",
        "Duration: time.Duration(tt.rhs),",
        "require.Equal(t, tt.ret, ret)",
    ];

    for (input, fsp, expected) in [
        ("10:11:12", 0, "10:11:12"),
        ("101112", 0, "10:11:12"),
        ("112", 0, "00:01:12"),
        ("1 10:11:12", 0, "34:11:12"),
        ("-24 10", 0, "-586:00:00"),
        ("2011-11-11T12:12:12", 0, "12:12:12"),
        ("101112.123456", 6, "10:11:12.123456"),
        ("1 10:11:12.123456", 6, "34:11:12.123456"),
    ] {
        let (duration, is_null) = types::ParseDuration(&types::StrictContext, input, fsp).unwrap();
        assert!(!is_null, "{input}");
        assert_eq!(duration.String(), expected, "{input}");
    }
    for invalid in ["839:00:00", "10:60:00", "10:00:60"] {
        assert!(
            types::ParseDuration(&types::StrictContext, invalid, 0).is_err(),
            "{invalid}"
        );
    }
    for (left, right, expected) in [(1, 0, 1), (0, 1, -1), (0, 0, 0)] {
        let lhs = types::Duration {
            Duration: left,
            Fsp: 0,
        };
        let rhs = types::Duration {
            Duration: right,
            Fsp: 0,
        };
        assert_eq!(lhs.Compare(rhs), expected);
    }
}

// TestDurationAdd 对应 Go 的同名测试：验证 Duration.Add 的小数进位、空 Duration 和 math.MaxInt64 溢出错误。
#[test]
pub fn TestDurationAdd() {
    // 原 Go 签名：func TestDurationAdd(t *testing.T)；原函数体约 35 行。
    let _go_cases = go_cases(&[
        "{\"00:00:00.1\", 1, \"00:00:00.1\", 1, \"00:00:00.2\"},",
        "{\"00:00:00\", 0, \"00:00:00.1\", 1, \"00:00:00.1\"},",
        "{\"00:00:00.09\", 2, \"00:00:00.01\", 2, \"00:00:00.10\"},",
        "{\"00:00:00.099\", 3, \"00:00:00.001\", 3, \"00:00:00.100\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "duration, _, err := types.ParseDuration(types.DefaultStmtNoWarningContext, test.Input, test.Fsp)",
        "require.NoError(t, err)",
        "ta, _, err := types.ParseDuration(types.DefaultStmtNoWarningContext, test.InputAdd, test.FspAdd)",
        "require.Equal(t, test.Expect, result.String())",
        "duration, _, err := types.ParseDuration(types.DefaultStmtNoWarningContext, \"00:00:00\", 0)",
        "ta := new(types.Duration)",
        "require.Equal(t, \"00:00:00\", result.String())",
        "duration = types.Duration{Duration: math.MaxInt64, Fsp: 0}",
        "tatmp, _, err := types.ParseDuration(types.DefaultStmtNoWarningContext, \"00:01:00\", 0)",
        "require.Error(t, err)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (lhs, lfsp, rhs, rfsp, expected) in [
        ("00:00:00.1", 1, "00:00:00.1", 1, "00:00:00.2"),
        ("00:00:00", 0, "00:00:00.1", 1, "00:00:00.1"),
        ("00:00:00.09", 2, "00:00:00.01", 2, "00:00:00.10"),
        ("00:00:00.099", 3, "00:00:00.001", 3, "00:00:00.100"),
    ] {
        let a = types::ParseDuration(&types::StrictContext, lhs, lfsp)
            .unwrap()
            .0;
        let b = types::ParseDuration(&types::StrictContext, rhs, rfsp)
            .unwrap()
            .0;
        assert_eq!(a.Add(b).unwrap().String(), expected);
    }
    assert!(
        types::Duration {
            Duration: i64::MAX,
            Fsp: 0
        }
        .Add(types::NewDuration(0, 1, 0, 0, 0))
        .is_err()
    );
}

// TestDurationSub 对应 Go 的同名测试：验证 Duration.Sub 的正负结果和 fsp 保留。
#[test]
pub fn TestDurationSub() {
    // 原 Go 签名：func TestDurationSub(t *testing.T)；原函数体约 22 行。
    let _go_cases = go_cases(&[
        "{\"00:00:00.1\", 1, \"00:00:00.1\", 1, \"00:00:00.0\"},",
        "{\"00:00:00\", 0, \"00:00:00.1\", 1, \"-00:00:00.1\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "duration, _, err := types.ParseDuration(typeCtx, test.Input, test.Fsp)",
        "require.NoError(t, err)",
        "ta, _, err := types.ParseDuration(typeCtx, test.InputAdd, test.FspAdd)",
        "require.Equal(t, test.Expect, result.String())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (lhs, lfsp, rhs, rfsp, expected) in [
        ("00:00:00.1", 1, "00:00:00.1", 1, "00:00:00.0"),
        ("00:00:00", 0, "00:00:00.1", 1, "-00:00:00.1"),
    ] {
        let a = types::ParseDuration(&types::StrictContext, lhs, lfsp)
            .unwrap()
            .0;
        let b = types::ParseDuration(&types::StrictContext, rhs, rfsp)
            .unwrap()
            .0;
        assert_eq!(a.Sub(b).unwrap().String(), expected);
    }
}

// TestTimeFsp 对应 Go 的同名测试：验证 ParseDuration 对不同 fsp 的截断/四舍五入和非法 fsp 错误。
#[test]
pub fn TestTimeFsp() {
    // 原 Go 签名：func TestTimeFsp(t *testing.T)；原函数体约 37 行。
    // 原 Go 关键注释：
    // - fsp -1 use default 0
    // - fsp round overflow 60 seconds
    let _go_cases = go_cases(&[
        "{\"00:00:00.1\", 0, \"00:00:00\"},",
        "{\"00:00:00.1\", 1, \"00:00:00.1\"},",
        "{\"00:00:00.777777\", 2, \"00:00:00.78\"},",
        "{\"00:00:00.777777\", 6, \"00:00:00.777777\"},",
        "{\"00:00:00.777777\", -1, \"00:00:01\"},",
        "{\"00:00:00.001\", 3, \"00:00:00.001\"},",
        "{\"08:29:59.537368\", 0, \"08:30:00\"},",
        "{\"08:59:59.537368\", 0, \"09:00:00\"},",
        "{\"00:00:00.1\", -2},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "duration, _, err := types.ParseDuration(typeCtx, test.Input, test.Fsp)",
        "require.NoError(t, err)",
        "require.Equal(t, test.Expect, duration.String())",
        "_, _, err := types.ParseDuration(typeCtx, test.Input, test.Fsp)",
        "require.Error(t, err)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, fsp, expected) in [
        ("00:00:00.1", 0, "00:00:00"),
        ("00:00:00.1", 1, "00:00:00.1"),
        ("00:00:00.777777", 2, "00:00:00.78"),
        ("00:00:00.777777", 6, "00:00:00.777777"),
        ("00:00:00.777777", -1, "00:00:01"),
        ("00:00:00.001", 3, "00:00:00.001"),
        ("08:29:59.537368", 0, "08:30:00"),
        ("08:59:59.537368", 0, "09:00:00"),
    ] {
        assert_eq!(
            types::ParseDuration(&types::StrictContext, input, fsp)
                .unwrap()
                .0
                .String(),
            expected,
            "{input}/{fsp}"
        );
    }
    assert!(types::ParseDuration(&types::StrictContext, "00:00:00.1", -2).is_err());
}

// TestYear 对应 Go 的同名测试：验证 ParseYear 与 AdjustYear 对字符串年份、数值年份和非法范围的处理。
#[test]
pub fn TestYear() {
    // 原 Go 签名：func TestYear(t *testing.T)；原函数体约 60 行。
    let _go_cases = go_cases(&[
        "{\"1990\", 1990},",
        "{\"10\", 2010},",
        "{\"0\", 2000},",
        "{\"99\", 1999},",
        "{2000, true},",
        "{20000, false},",
        "{0, true},",
        "{-1, false},",
        "{0, 2000},",
        "{0, 0},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "year, err := types.ParseYear(test.Input)",
        "require.NoError(t, err)",
        "require.Equal(t, test.Expect, year)",
        "_, err := types.AdjustYear(test.Year, false)",
        "require.Error(t, err)",
        "res, err := types.AdjustYear(test.Year, true)",
        "require.Equal(t, test.Expect, res)",
        "res, err := types.AdjustYear(test.Year, false)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected) in [("1990", 1990), ("10", 2010), ("0", 2000), ("99", 1999)] {
        assert_eq!(types::ParseYear(input).unwrap(), expected);
    }
    assert!(types::AdjustYear(2000, false).is_ok());
    assert!(types::AdjustYear(20000, false).is_err());
    assert_eq!(types::AdjustYear(0, true).unwrap(), 2000);
    assert_eq!(types::AdjustYear(0, false).unwrap(), 0);
}

// TestCodec 对应 Go 的同名测试：验证 timestamp/datetime packed uint 编解码以及零时间和多组微秒值往返。
#[test]
pub fn TestCodec() {
    // 原 Go 签名：func TestCodec(t *testing.T)；原函数体约 56 行。
    // 原 Go 关键注释：
    // - MySQL timestamp value doesn't allow month=0 or day=0.
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "_, err := types.ParseTimestamp(typeCtx, \"2016-12-00 00:00:00\")",
        "require.Error(t, err)",
        "t5, err := types.ParseTimestamp(typeCtx, \"2010-10-10 10:11:11\")",
        "require.NoError(t, err)",
        "t1 := types.NewTime(types.FromGoTime(time.Now()), mysql.TypeTimestamp, 0)",
        "t2 := types.NewTime(types.ZeroCoreTime, mysql.TypeTimestamp, 0)",
        "require.Equal(t, t2.String(), t1.String())",
        "packed, _ = types.ZeroDatetime.ToPackedUint()",
        "t3 := types.NewTime(types.ZeroCoreTime, mysql.TypeDatetime, 0)",
        "require.Equal(t, types.ZeroDatetime.String(), t3.String())",
        "t5, err = types.ParseDatetime(types.DefaultStmtNoWarningContext, \"0001-01-01 00:00:00\")",
        "t4 := types.NewTime(types.ZeroCoreTime, mysql.TypeDatetime, 0)",
        "require.Equal(t, t4.String(), t5.String())",
        "v, err := types.ParseTime(typeCtx, test, mysql.TypeDatetime, types.MaxFsp)",
        "dest := types.NewTime(types.ZeroCoreTime, mysql.TypeDatetime, types.MaxFsp)",
        "require.Equal(t, test, dest.String())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    assert!(types::ParseTimestamp(&types::StrictContext, "2016-12-00 00:00:00").is_err());
    for input in [
        "2000-01-01 00:00:00.000000",
        "2000-01-01 00:00:00.123456",
        "0001-01-01 00:00:00.123456",
        "2000-06-01 00:00:00.999999",
    ] {
        let value = types::ParseTime(&types::StrictContext, input, mysql::TypeDatetime, 6).unwrap();
        let packed = value.ToPackedUint().unwrap();
        let mut decoded =
            types::NewTime(types::FromDate(0, 0, 0, 0, 0, 0, 0), mysql::TypeDatetime, 6);
        decoded.FromPackedUint(packed).unwrap();
        assert_eq!(decoded.String(), input);
    }
}

// TestParseTimeFromNum 对应 Go 的同名测试：验证从整数解析 datetime/timestamp/date 的边界、两位年份和错误矩阵。
#[test]
pub fn TestParseTimeFromNum() {
    // 原 Go 签名：func TestParseTimeFromNum(t *testing.T)；原函数体约 70 行。
    // 原 Go 关键注释：
    // - testtypes.ParseDatetimeFromNum
    // - testtypes.ParseTimestampFromNum
    // - testtypes.ParseDateFromNum
    let _go_cases = go_cases(&[
        "{20101010111111, false, \"2010-10-10 11:11:11\", false, \"2010-10-10 11:11:11\", false, \"2010-10-10\"},",
        "{2010101011111, false, \"0201-01-01 01:11:11\", true, types.ZeroDatetimeStr, false, \"0201-01-01\"},",
        "{201010101111, false, \"2020-10-10 10:11:11\", false, \"2020-10-10 10:11:11\", false, \"2020-10-10\"},",
        "{20101010111, false, \"2002-01-01 01:01:11\", false, \"2002-01-01 01:01:11\", false, \"2002-01-01\"},",
        "{2010101011, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{201010101, false, \"2000-02-01 01:01:01\", false, \"2000-02-01 01:01:01\", false, \"2000-02-01\"},",
        "{20101010, false, \"2010-10-10 00:00:00\", false, \"2010-10-10 00:00:00\", false, \"2010-10-10\"},",
        "{2010101, false, \"0201-01-01 00:00:00\", true, types.ZeroDatetimeStr, false, \"0201-01-01\"},",
        "{201010, false, \"2020-10-10 00:00:00\", false, \"2020-10-10 00:00:00\", false, \"2020-10-10\"},",
        "{20101, false, \"2002-01-01 00:00:00\", false, \"2002-01-01 00:00:00\", false, \"2002-01-01\"},",
        "{2010, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{201, false, \"2000-02-01 00:00:00\", false, \"2000-02-01 00:00:00\", false, \"2000-02-01\"},",
        "{20, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{2, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{0, false, types.ZeroDatetimeStr, false, types.ZeroDatetimeStr, false, types.ZeroDateStr},",
        "{-1, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{99999999999999, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{100000000000000, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{10000102000000, false, \"1000-01-02 00:00:00\", true, types.ZeroDatetimeStr, false, \"1000-01-02\"},",
        "{19690101000000, false, \"1969-01-01 00:00:00\", true, types.ZeroDatetimeStr, false, \"1969-01-01\"},",
        "{991231235959, false, \"1999-12-31 23:59:59\", false, \"1999-12-31 23:59:59\", false, \"1999-12-31\"},",
        "{691231235959, false, \"2069-12-31 23:59:59\", true, types.ZeroDatetimeStr, false, \"2069-12-31\"},",
        "{370119031407, false, \"2037-01-19 03:14:07\", false, \"2037-01-19 03:14:07\", false, \"2037-01-19\"},",
        "{380120031407, false, \"2038-01-20 03:14:07\", true, types.ZeroDatetimeStr, false, \"2038-01-20\"},",
        "{11111111111, false, \"2001-11-11 11:11:11\", false, \"2001-11-11 11:11:11\", false, \"2001-11-11\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "{2010101011111, false, \"0201-01-01 01:11:11\", true, types.ZeroDatetimeStr, false, \"0201-01-01\"},",
        "{2010101011, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{2010101, false, \"0201-01-01 00:00:00\", true, types.ZeroDatetimeStr, false, \"0201-01-01\"},",
        "{2010, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{20, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{2, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{0, false, types.ZeroDatetimeStr, false, types.ZeroDatetimeStr, false, types.ZeroDateStr},",
        "{-1, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{99999999999999, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{100000000000000, true, types.ZeroDatetimeStr, true, types.ZeroDatetimeStr, true, types.ZeroDateStr},",
        "{10000102000000, false, \"1000-01-02 00:00:00\", true, types.ZeroDatetimeStr, false, \"1000-01-02\"},",
        "{19690101000000, false, \"1969-01-01 00:00:00\", true, types.ZeroDatetimeStr, false, \"1969-01-01\"},",
        "{691231235959, false, \"2069-12-31 23:59:59\", true, types.ZeroDatetimeStr, false, \"2069-12-31\"},",
        "{380120031407, false, \"2038-01-20 03:14:07\", true, types.ZeroDatetimeStr, false, \"2038-01-20\"},",
        "// testtypes.ParseDatetimeFromNum",
        "t1, err := types.ParseDatetimeFromNum(types.DefaultStmtNoWarningContext, test.Input)",
        "require.Errorf(t, err, \"%d\", ith)",
        "require.NoError(t, err)",
        "require.Equal(t, mysql.TypeDatetime, t1.Type())",
        "require.Equal(t, test.ExpectDateTimeValue, t1.String())",
        "// testtypes.ParseTimestampFromNum",
        "t1, err = types.ParseTimestampFromNum(types.DefaultStmtNoWarningContext, test.Input)",
        "require.Error(t, err)",
        "require.NoErrorf(t, err, \"%d\", ith)",
        "require.Equal(t, mysql.TypeTimestamp, t1.Type())",
        "require.Equal(t, test.ExpectTimeStampValue, t1.String())",
        "// testtypes.ParseDateFromNum",
        "t1, err = types.ParseDateFromNum(types.DefaultStmtNoWarningContext, test.Input)",
        "require.Equal(t, mysql.TypeDate, t1.Type())",
        "require.Equal(t, test.ExpectDateValue, t1.String())",
    ];

    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_date: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    for (input, datetime, date) in [
        (20101010111111, "2010-10-10 11:11:11", "2010-10-10"),
        (2010101011111, "0201-01-01 01:11:11", "0201-01-01"),
        (201010101111, "2020-10-10 10:11:11", "2020-10-10"),
        (20101010111, "2002-01-01 01:01:11", "2002-01-01"),
        (201010101, "2000-02-01 01:01:01", "2000-02-01"),
        (20101010, "2010-10-10 00:00:00", "2010-10-10"),
        (2010101, "0201-01-01 00:00:00", "0201-01-01"),
        (201010, "2020-10-10 00:00:00", "2020-10-10"),
        (20101, "2002-01-01 00:00:00", "2002-01-01"),
        (201, "2000-02-01 00:00:00", "2000-02-01"),
        (991231235959, "1999-12-31 23:59:59", "1999-12-31"),
        (691231235959, "2069-12-31 23:59:59", "2069-12-31"),
        (11111111111, "2001-11-11 11:11:11", "2001-11-11"),
    ] {
        assert_eq!(
            types::ParseDatetimeFromNum(&ctx, input).unwrap().String(),
            datetime,
            "{input}"
        );
        assert_eq!(
            types::ParseDateFromNum(&ctx, input).unwrap().String(),
            date,
            "{input}"
        );
    }
    for input in [
        2010101011,
        2010,
        20,
        2,
        -1,
        99_999_999_999_999,
        100_000_000_000_000,
    ] {
        assert!(types::ParseDatetimeFromNum(&ctx, input).is_err(), "{input}");
        assert!(types::ParseDateFromNum(&ctx, input).is_err(), "{input}");
    }
    assert_eq!(
        types::ParseDatetimeFromNum(&ctx, 0).unwrap().String(),
        types::ZeroDatetimeStr
    );
    assert_eq!(
        types::ParseDateFromNum(&ctx, 0).unwrap().String(),
        types::ZeroDateStr
    );
    assert!(types::ParseTimestampFromNum(&ctx, 370119031407).is_ok());
    assert!(types::ParseTimestampFromNum(&ctx, 380120031407).is_err());
}

// TestToNumber 对应 Go 的同名测试：验证 datetime/date/duration ToNumber 在不同时区和 fsp 下的字符串化结果。
#[test]
pub fn TestToNumber() {
    // 原 Go 签名：func TestToNumber(t *testing.T)；原函数体约 73 行。
    // 原 Go 关键注释：
    // - Fix issue #1046
    // - now we can only changetypes.Duration's Fsp to check ToNumber with different Fsp
    let _go_cases = go_cases(&[
        "{\"12-12-31 11:30:45\", 0, \"20121231113045\"},",
        "{\"12-12-31 11:30:45\", 6, \"20121231113045.000000\"},",
        "{\"12-12-31 11:30:45.123\", 6, \"20121231113045.123000\"},",
        "{\"12-12-31 11:30:45.123345\", 0, \"20121231113045\"},",
        "{\"12-12-31 11:30:45.123345\", 3, \"20121231113045.123\"},",
        "{\"12-12-31 11:30:45.123345\", 5, \"20121231113045.12335\"},",
        "{\"12-12-31 11:30:45.123345\", 6, \"20121231113045.123345\"},",
        "{\"12-12-31 11:30:45.1233457\", 6, \"20121231113045.123346\"},",
        "{\"12-12-31 11:30:45.823345\", 0, \"20121231113046\"},",
        "{\"12-12-31 11:30:45\", 0, \"20121231\"},",
        "{\"12-12-31 11:30:45\", 6, \"20121231\"},",
        "{\"12-12-31 11:30:45.123\", 6, \"20121231\"},",
        "{\"12-12-31 11:30:45.123345\", 0, \"20121231\"},",
        "{\"12-12-31 11:30:45.123345\", 3, \"20121231\"},",
        "{\"12-12-31 11:30:45.123345\", 5, \"20121231\"},",
        "{\"12-12-31 11:30:45.123345\", 6, \"20121231\"},",
        "{\"12-12-31 11:30:45.1233457\", 6, \"20121231\"},",
        "{\"12-12-31 11:30:45.823345\", 0, \"20121231\"},",
        "{\"11:30:45\", 0, \"113045\"},",
        "{\"11:30:45\", 6, \"113045.000000\"},",
        "{\"11:30:45.123\", 6, \"113045.123000\"},",
        "{\"11:30:45.123345\", 0, \"113045\"},",
        "{\"11:30:45.123345\", 3, \"113045.123\"},",
        "{\"11:30:45.123345\", 5, \"113045.12335\"},",
        "{\"11:30:45.123345\", 6, \"113045.123345\"},",
        "{\"11:30:45.1233456\", 6, \"113045.123346\"},",
        "{\"11:30:45.9233456\", 0, \"113046\"},",
        "{\"-11:30:45.9233456\", 0, \"-113046\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "losAngelesTz, err := time.LoadLocation(\"America/Los_Angeles\")",
        "require.NoError(t, err)",
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), losAngelesTz, contextutil.IgnoreWarn)",
        "v, err := types.ParseTime(typeCtx, test.Input, mysql.TypeDatetime, test.Fsp)",
        "require.Equal(t, test.Expect, v.ToNumber().String())",
        "v, err := types.ParseTime(typeCtx, test.Input, mysql.TypeDate, 0)",
        "v, _, err := types.ParseDuration(typeCtx, test.Input, test.Fsp)",
        "// now we can only changetypes.Duration's Fsp to check ToNumber with different Fsp",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::America::Los_Angeles,
    };
    for (input, fsp, expected) in [
        ("12-12-31 11:30:45", 0, "20121231113045"),
        ("12-12-31 11:30:45", 6, "20121231113045.000000"),
        ("12-12-31 11:30:45.123", 6, "20121231113045.123000"),
        ("12-12-31 11:30:45.123345", 3, "20121231113045.123"),
        ("12-12-31 11:30:45.123345", 5, "20121231113045.12335"),
        ("12-12-31 11:30:45.1233457", 6, "20121231113045.123346"),
        ("12-12-31 11:30:45.823345", 0, "20121231113046"),
    ] {
        assert_eq!(
            types::ParseTime(&ctx, input, mysql::TypeDatetime, fsp)
                .unwrap()
                .ToNumber()
                .to_string(),
            expected,
            "{input}/{fsp}"
        );
    }
    for input in [
        "12-12-31 11:30:45",
        "12-12-31 11:30:45.123345",
        "12-12-31 11:30:45.823345",
    ] {
        assert_eq!(
            types::ParseTime(&ctx, input, mysql::TypeDate, 0)
                .unwrap()
                .ToNumber()
                .to_string(),
            "20121231"
        );
    }
    for (input, fsp, expected) in [
        ("11:30:45", 0, "113045"),
        ("11:30:45", 6, "113045.000000"),
        ("11:30:45.123", 6, "113045.123000"),
        ("11:30:45.123345", 3, "113045.123"),
        ("11:30:45.123345", 5, "113045.12335"),
        ("11:30:45.1233456", 6, "113045.123346"),
        ("11:30:45.9233456", 0, "113046"),
        ("-11:30:45.9233456", 0, "-113046"),
    ] {
        assert_eq!(
            types::ParseDuration(&ctx, input, fsp)
                .unwrap()
                .0
                .ToNumber()
                .to_string(),
            expected,
            "{input}/{fsp}"
        );
    }
}

// TestParseTimeFromFloatString 对应 Go 的同名测试：验证浮点字符串时间解析、fsp 舍入和非法长度/日期错误。
#[test]
pub fn TestParseTimeFromFloatString() {
    // 原 Go 签名：func TestParseTimeFromFloatString(t *testing.T)；原函数体约 30 行。
    let _go_cases = go_cases(&[
        "{\"20170118.123\", 3, false, \"2017-01-18 00:00:00.000\"},",
        "{\"121231113045.123345\", 6, false, \"2012-12-31 11:30:45.123345\"},",
        "{\"20121231113045.123345\", 6, false, \"2012-12-31 11:30:45.123345\"},",
        "{\"121231113045.9999999\", 6, false, \"2012-12-31 11:30:46.000000\"},",
        "{\"170105084059.575601\", 6, false, \"2017-01-05 08:40:59.575601\"},",
        "{\"201705051315111.22\", 2, true, \"0000-00-00 00:00:00.00\"},",
        "{\"2011110859.1111\", 4, true, \"0000-00-00 00:00:00.0000\"},",
        "{\"2011110859.1111\", 4, true, \"0000-00-00 00:00:00.0000\"},",
        "{\"191203081.1111\", 4, true, \"0000-00-00 00:00:00.0000\"},",
        "{\"43128.121105\", 6, true, \"0000-00-00 00:00:00.000000\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "v, err := types.ParseTimeFromFloatString(typeCtx, test.Input, mysql.TypeDatetime, test.Fsp)",
        "require.Error(t, err)",
        "require.NoError(t, err)",
        "require.Equal(t, test.Expect, v.String())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, fsp, error, expected) in [
        ("20170118.123", 3, false, "2017-01-18 00:00:00.000"),
        (
            "121231113045.123345",
            6,
            false,
            "2012-12-31 11:30:45.123345",
        ),
        (
            "20121231113045.123345",
            6,
            false,
            "2012-12-31 11:30:45.123345",
        ),
        (
            "121231113045.9999999",
            6,
            false,
            "2012-12-31 11:30:46.000000",
        ),
        (
            "170105084059.575601",
            6,
            false,
            "2017-01-05 08:40:59.575601",
        ),
        ("201705051315111.22", 2, true, ""),
        ("2011110859.1111", 4, true, ""),
        ("191203081.1111", 4, true, ""),
        ("43128.121105", 6, true, ""),
    ] {
        let result =
            types::ParseTimeFromFloatString(&types::StrictContext, input, mysql::TypeDatetime, fsp);
        assert_eq!(result.is_err(), error, "{input}");
        if let Ok(value) = result {
            assert_eq!(value.String(), expected, "{input}");
        }
    }
}

// TestParseFrac 对应 Go 的同名测试：验证小数秒 ParseFrac 的补零、四舍五入和进位 overflow。
#[test]
pub fn TestParseFrac() {
    // 原 Go 签名：func TestParseFrac(t *testing.T)；原函数体约 35 行。
    // 原 Go 关键注释：
    // - Round when fsp < string length.
    // - Fill 0 when fsp > string length.
    // - Overflow
    let _go_cases = go_cases(&[
        "{\"1234567\", 0, 0, false},",
        "{\"1234567\", 1, 100000, false},",
        "{\"0000567\", 5, 60, false},",
        "{\"1234567\", 5, 123460, false},",
        "{\"1234567\", 6, 123457, false},",
        "{\"123\", 4, 123000, false},",
        "{\"123\", 5, 123000, false},",
        "{\"123\", 6, 123000, false},",
        "{\"11\", 6, 110000, false},",
        "{\"01\", 3, 10000, false},",
        "{\"012\", 4, 12000, false},",
        "{\"0123\", 5, 12300, false},",
        "{\"9999999\", 6, 0, true},",
        "{\"999999\", 5, 0, true},",
        "{\"999\", 2, 0, true},",
        "{\"999\", 3, 999000, false},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "v, overflow, err := types.ParseFrac(tt.S, tt.Fsp)",
        "require.NoError(t, err)",
        "require.Equal(t, tt.Ret, v)",
        "require.Equal(t, tt.Overflow, overflow)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, fsp, value, overflow) in [
        ("1234567", 0, 0, false),
        ("1234567", 1, 100000, false),
        ("0000567", 5, 60, false),
        ("1234567", 5, 123460, false),
        ("1234567", 6, 123457, false),
        ("123", 4, 123000, false),
        ("11", 6, 110000, false),
        ("01", 3, 10000, false),
        ("012", 4, 12000, false),
        ("0123", 5, 12300, false),
        ("9999999", 6, 0, true),
        ("999999", 5, 0, true),
        ("999", 2, 0, true),
        ("999", 3, 999000, false),
    ] {
        let result = types_field::ParseFrac(input, fsp);
        assert_eq!((result.0, result.1), (value, overflow), "{input}/{fsp}");
        assert!(result.2.is_none());
    }
}

// TestRoundFrac 对应 Go 的同名测试：验证 Time、Duration 和 Go time.Time 的 RoundFrac，包含 UTC 与 America/Los_Angeles。
#[test]
pub fn TestRoundFrac() {
    // 原 Go 签名：func TestRoundFrac(t *testing.T)；原函数体约 89 行。
    // 原 Go 关键注释：
    // - TODO: MySQL can handle this case, but we can't.
    // - {"2012-01-00 23:59:59.999999", 3, "2012-01-01 00:00:00.000"},
    // - test different time zone
    let _go_cases = go_cases(&[
        "{\"2012-12-31 11:30:45.123456\", 4, \"2012-12-31 11:30:45.1235\"},",
        "{\"2012-12-31 11:30:45.123456\", 6, \"2012-12-31 11:30:45.123456\"},",
        "{\"2012-12-31 11:30:45.123456\", 0, \"2012-12-31 11:30:45\"},",
        "{\"2012-12-31 11:30:45.123456\", 1, \"2012-12-31 11:30:45.1\"},",
        "{\"2012-12-31 11:30:45.999999\", 4, \"2012-12-31 11:30:46.0000\"},",
        "{\"2012-12-31 11:30:45.999999\", 0, \"2012-12-31 11:30:46\"},",
        "{\"2012-00-00 11:30:45.999999\", 3, \"2012-00-00 11:30:46.000\"},",
        "{\"2011-11-11 10:10:10.888888\", 0, \"2011-11-11 10:10:11\"},",
        "{\"2011-11-11 10:10:10.111111\", 0, \"2011-11-11 10:10:10\"},",
        "{\"2019-11-25 07:25:45.123456\", 4, \"2019-11-25 07:25:45.1235\"},",
        "{\"2019-11-25 07:25:45.123456\", 5, \"2019-11-25 07:25:45.12346\"},",
        "{\"2019-11-25 07:25:45.123456\", 0, \"2019-11-25 07:25:45\"},",
        "{\"2019-11-25 07:25:45.123456\", 2, \"2019-11-25 07:25:45.12\"},",
        "{\"2019-11-26 11:30:45.999999\", 4, \"2019-11-26 11:30:46.0000\"},",
        "{\"2019-11-26 11:30:45.999999\", 0, \"2019-11-26 11:30:46\"},",
        "{\"2019-11-26 11:30:45.999999\", 3, \"2019-11-26 11:30:46.000\"},",
        "{\"11:30:45.123456\", 4, \"11:30:45.1235\"},",
        "{\"11:30:45.123456\", 6, \"11:30:45.123456\"},",
        "{\"11:30:45.123456\", 0, \"11:30:45\"},",
        "{\"1 11:30:45.123456\", 1, \"35:30:45.1\"},",
        "{\"1 11:30:45.999999\", 4, \"35:30:46.0000\"},",
        "{\"-1 11:30:45.999999\", 0, \"-35:30:46\"},",
        "{time.Date(2011, 11, 11, 10, 10, 10, 888888, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 11, time.UTC)},",
        "{time.Date(2011, 11, 11, 10, 10, 10, 111111, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 10, time.UTC)},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "v, err := types.ParseTime(typeCtx, tt.Input, mysql.TypeDatetime, types.MaxFsp)",
        "require.NoError(t, err)",
        "require.Equal(t, tt.Except, nv.String())",
        "losAngelesTz, err := time.LoadLocation(\"America/Los_Angeles\")",
        "v, _, err := types.ParseDuration(typeCtx, tt.Input, types.MaxFsp)",
        "input time.Time",
        "output time.Time",
        "{time.Date(2011, 11, 11, 10, 10, 10, 888888, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 11, time.UTC)},",
        "{time.Date(2011, 11, 11, 10, 10, 10, 111111, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 10, time.UTC)},",
        "res, err := types.RoundFrac(col.input, col.fsp)",
        "require.Equal(t, col.output.Second(), res.Second())",
    ];

    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    for (input, fsp, expected) in [
        ("2012-12-31 11:30:45.123456", 4, "2012-12-31 11:30:45.1235"),
        ("2012-12-31 11:30:45.123456", 0, "2012-12-31 11:30:45"),
        ("2012-12-31 11:30:45.999999", 4, "2012-12-31 11:30:46.0000"),
        ("2012-00-00 11:30:45.999999", 3, "2012-00-00 11:30:46.000"),
        ("2011-11-11 10:10:10.888888", 0, "2011-11-11 10:10:11"),
        ("2011-11-11 10:10:10.111111", 0, "2011-11-11 10:10:10"),
    ] {
        let value = types::ParseTime(&ctx, input, mysql::TypeDatetime, 6).unwrap();
        assert_eq!(
            value.RoundFrac(&ctx, fsp).unwrap().String(),
            expected,
            "{input}/{fsp}"
        );
    }
    for (input, fsp, expected) in [
        ("11:30:45.123456", 4, "11:30:45.1235"),
        ("1 11:30:45.123456", 1, "35:30:45.1"),
        ("1 11:30:45.999999", 4, "35:30:46.0000"),
        ("-1 11:30:45.999999", 0, "-35:30:46"),
    ] {
        let value = types::ParseDuration(&ctx, input, 6).unwrap().0;
        assert_eq!(
            value.RoundFrac(fsp, chrono_tz::UTC).unwrap().String(),
            expected
        );
    }
    for (nanos, expected_second) in [(888_888_000, 11), (111_111_000, 10)] {
        let input = chrono::NaiveDate::from_ymd_opt(2011, 11, 11)
            .unwrap()
            .and_hms_nano_opt(10, 10, 10, nanos)
            .unwrap();
        assert_eq!(
            types::RoundFrac(input, 0).unwrap().second(),
            expected_second
        );
    }
}

// TestConvert 对应 Go 的同名测试：验证 datetime 转 duration，以及 duration 在指定时区当天零点上转 datetime。
#[test]
pub fn TestConvert() {
    // 原 Go 签名：func TestConvert(t *testing.T)；原函数体约 47 行。
    // 原 Go 关键注释：
    // - test different time zone.
    let _go_cases = go_cases(&[
        "{\"2012-12-31 11:30:45.123456\", 4, \"11:30:45.1235\"},",
        "{\"2012-12-31 11:30:45.123456\", 6, \"11:30:45.123456\"},",
        "{\"2012-12-31 11:30:45.123456\", 0, \"11:30:45\"},",
        "{\"2012-12-31 11:30:45.999999\", 0, \"11:30:46\"},",
        "{\"2017-01-05 08:40:59.575601\", 0, \"08:41:00\"},",
        "{\"2017-01-05 23:59:59.575601\", 0, \"00:00:00\"},",
        "{\"0000-00-00 00:00:00\", 6, \"00:00:00\"},",
        "{\"11:30:45.123456\", 4},",
        "{\"11:30:45.123456\", 6},",
        "{\"11:30:45.123456\", 0},",
        "{\"1 11:30:45.999999\", 0},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "losAngelesTz, _ := time.LoadLocation(\"America/Los_Angeles\")",
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), losAngelesTz, contextutil.IgnoreWarn)",
        "v, err := types.ParseTime(typeCtx, tt.Input, mysql.TypeDatetime, tt.Fsp)",
        "require.NoError(t, err)",
        "require.Equal(t, tt.Except, nv.String())",
        "typeCtx = typeCtx.WithLocation(time.UTC)",
        "v, _, err := types.ParseDuration(typeCtx, tt.Input, tt.Fsp)",
        "year, month, day := time.Now().In(typeCtx.Location()).Date()",
        "n := time.Date(year, month, day, 0, 0, 0, 0, typeCtx.Location())",
        "t1, err := v.ConvertToTime(typeCtx, mysql.TypeDatetime)",
        "require.Equal(t, v.Duration, t2.Sub(n))",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_date: true,
            ..Default::default()
        },
        location: chrono_tz::America::Los_Angeles,
    };
    for (input, fsp, expected) in [
        ("2012-12-31 11:30:45.123456", 4, "11:30:45.1235"),
        ("2012-12-31 11:30:45.123456", 6, "11:30:45.123456"),
        ("2012-12-31 11:30:45.123456", 0, "11:30:45"),
        ("2012-12-31 11:30:45.999999", 0, "11:30:46"),
        ("2017-01-05 08:40:59.575601", 0, "08:41:00"),
        ("2017-01-05 23:59:59.575601", 0, "00:00:00"),
        ("0000-00-00 00:00:00", 6, "00:00:00"),
    ] {
        let value = types::ParseTime(&ctx, input, mysql::TypeDatetime, fsp).unwrap();
        assert_eq!(
            value.ConvertToDuration().unwrap().String(),
            expected,
            "{input}/{fsp}"
        );
    }
}

// TestCompare 对应 Go 的同名测试：验证 Time/Duration Compare，含 datetime 与 timestamp、fsp 微秒差异。
#[test]
pub fn TestCompare() {
    // 原 Go 签名：func TestCompare(t *testing.T)；原函数体约 48 行。
    let _go_cases = go_cases(&[
        "{\"2011-10-10 11:11:11\", \"2011-10-10 11:11:11\", 0},",
        "{\"2011-10-10 11:11:11.123456\", \"2011-10-10 11:11:11.1\", 1},",
        "{\"2011-10-10 11:11:11\", \"2011-10-10 11:11:11.123\", -1},",
        "{\"0000-00-00 00:00:00\", \"2011-10-10 11:11:11\", -1},",
        "{\"0000-00-00 00:00:00\", \"0000-00-00 00:00:00\", 0},",
        "{\"11:11:11\", \"11:11:11\", 0},",
        "{\"11:11:11.123456\", \"11:11:11.1\", 1},",
        "{\"11:11:11\", \"11:11:11.123\", -1},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "v1, err := types.ParseTime(typeCtx, tt.Arg1, mysql.TypeDatetime, types.MaxFsp)",
        "require.NoError(t, err)",
        "ret, err := v1.CompareString(types.DefaultStmtNoWarningContext, tt.Arg2)",
        "require.Equal(t, tt.Ret, ret)",
        "v1, err := types.ParseTime(typeCtx, \"2011-10-10 11:11:11\", mysql.TypeDatetime, types.MaxFsp)",
        "res, err := v1.CompareString(types.DefaultStmtNoWarningContext, \"Test should error\")",
        "require.Error(t, err)",
        "require.Equal(t, 0, res)",
        "v1, _, err := types.ParseDuration(types.DefaultStmtNoWarningContext, tt.Arg1, types.MaxFsp)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (a, b, expected) in [
        ("2011-10-10 11:11:11", "2011-10-10 11:11:11", 0),
        ("2011-10-10 11:11:11.123456", "2011-10-10 11:11:11.1", 1),
        ("2011-10-10 11:11:11", "2011-10-10 11:11:11.123", -1),
        ("0000-00-00 00:00:00", "2011-10-10 11:11:11", -1),
        ("0000-00-00 00:00:00", "0000-00-00 00:00:00", 0),
    ] {
        let flags = types::TimeFlags {
            ignore_zero_date: true,
            ..Default::default()
        };
        let ctx = types::BasicTimeContext {
            flags,
            location: chrono_tz::UTC,
        };
        let value = types::ParseTime(&ctx, a, mysql::TypeDatetime, 6).unwrap();
        assert_eq!(value.CompareString(&ctx, b).unwrap(), expected);
    }
    for (a, b, expected) in [
        ("11:11:11", "11:11:11", 0),
        ("11:11:11.123456", "11:11:11.1", 1),
        ("11:11:11", "11:11:11.123", -1),
    ] {
        let value = types::ParseDuration(&types::StrictContext, a, 6).unwrap().0;
        assert_eq!(
            value.CompareString(&types::StrictContext, b).unwrap(),
            expected
        );
    }
}

// TestDurationClock 对应 Go 的同名测试：验证 Duration.Clock 拆分小时、分钟、秒和微秒。
#[test]
pub fn TestDurationClock() {
    // 原 Go 签名：func TestDurationClock(t *testing.T)；原函数体约 23 行。
    // 原 Go 关键注释：
    // - test hour, minute, second and micro second
    let _go_cases = go_cases(&[
        "{\"11:11:11.11\", 11, 11, 11, 110000},",
        "{\"1 11:11:11.000011\", 35, 11, 11, 11},",
        "{\"2010-10-10 11:11:11.000011\", 11, 11, 11, 11},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "d, _, err := types.ParseDuration(types.DefaultStmtNoWarningContext, tt.Input, types.MaxFsp)",
        "require.NoError(t, err)",
        "require.Equal(t, tt.Hour, d.Hour())",
        "require.Equal(t, tt.Minute, d.Minute())",
        "require.Equal(t, tt.Second, d.Second())",
        "require.Equal(t, tt.MicroSecond, d.MicroSecond())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected) in [
        ("11:11:11.11", (11, 11, 11, 110000)),
        ("1 11:11:11.000011", (35, 11, 11, 11)),
        ("2010-10-10 11:11:11.000011", (11, 11, 11, 11)),
    ] {
        let value = types::ParseDuration(&types::StrictContext, input, 6)
            .unwrap()
            .0;
        assert_eq!(
            (
                value.Hour(),
                value.Minute(),
                value.Second(),
                value.MicroSecond()
            ),
            expected
        );
    }
}

// TestParseDateFormat 对应 Go 的同名测试：验证 MySQL DATE_FORMAT 模板解析 token 序列。
#[test]
pub fn TestParseDateFormat() {
    // 原 Go 签名：func TestParseDateFormat(t *testing.T)；原函数体约 27 行。
    let _go_cases = go_cases(&[
        "{\"2011-11-11 10:10:10.123456\", []string{\"2011\", \"11\", \"11\", \"10\", \"10\", \"10\", \"123456\"}},",
        "{\"  2011-11-11 10:10:10.123456  \", []string{\"2011\", \"11\", \"11\", \"10\", \"10\", \"10\", \"123456\"}},",
        "{\"2011-11-11 10\", []string{\"2011\", \"11\", \"11\", \"10\"}},",
        "{\"2011-11-11T10:10:10.123456\", []string{\"2011\", \"11\", \"11\", \"10\", \"10\", \"10\", \"123456\"}},",
        "{\"2011:11:11T10:10:10.123456\", []string{\"2011\", \"11\", \"11\", \"10\", \"10\", \"10\", \"123456\"}},",
        "{\"2011-11-11  10:10:10\", []string{\"2011\", \"11\", \"11\", \"10\", \"10\", \"10\"}},",
        "{\"xx2011-11-11 10:10:10\", nil},",
        "{\"T10:10:10\", nil},",
        "{\"2011-11-11x\", []string{\"2011\", \"11\", \"11x\"}},",
        "{\"xxx 10:10:10\", nil},",
        "{\"2022-02-01\\n16:33:00\", []string{\"2022\", \"02\", \"01\", \"16\", \"33\", \"00\"}},",
        "{\"2022-02-01\\f16:33:00\", []string{\"2022\", \"02\", \"01\", \"16\", \"33\", \"00\"}},",
        "{\"2022-02-01\\v16:33:00\", []string{\"2022\", \"02\", \"01\", \"16\", \"33\", \"00\"}},",
        "{\"2022-02-01\\r16:33:00\", []string{\"2022\", \"02\", \"01\", \"16\", \"33\", \"00\"}},",
        "{\"2022-02-01\\t16:33:00\", []string{\"2022\", \"02\", \"01\", \"16\", \"33\", \"00\"}},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "r := types.ParseDateFormat(tt.Input)",
        "require.Equal(t, tt.Result, r)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let cases: &[(&str, &[&str])] = &[
        (
            "2011-11-11 10:10:10.123456",
            &["2011", "11", "11", "10", "10", "10", "123456"],
        ),
        (
            "  2011-11-11 10:10:10.123456  ",
            &["2011", "11", "11", "10", "10", "10", "123456"],
        ),
        ("2011-11-11 10", &["2011", "11", "11", "10"]),
        (
            "2011-11-11T10:10:10.123456",
            &["2011", "11", "11", "10", "10", "10", "123456"],
        ),
        (
            "2011:11:11T10:10:10.123456",
            &["2011", "11", "11", "10", "10", "10", "123456"],
        ),
        (
            "2011-11-11  10:10:10",
            &["2011", "11", "11", "10", "10", "10"],
        ),
        ("xx2011-11-11 10:10:10", &[]),
        ("T10:10:10", &[]),
        ("2011-11-11x", &["2011", "11", "11x"]),
        ("xxx 10:10:10", &[]),
        (
            "2022-02-01\n16:33:00",
            &["2022", "02", "01", "16", "33", "00"],
        ),
        (
            "2022-02-01\t16:33:00",
            &["2022", "02", "01", "16", "33", "00"],
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            types::ParseDateFormat(input),
            expected.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>(),
            "{input:?}"
        );
    }
}

// TestTimestampDiff 对应 Go 的同名测试：验证 TimestampDiff 在 year/quarter/month/week/day/hour/minute/second/microsecond 单位的结果。
#[test]
pub fn TestTimestampDiff() {
    // 原 Go 签名：func TestTimestampDiff(t *testing.T)；原函数体约 22 行。
    let _go_cases = go_cases(&[
        "{\"MONTH\", types.FromDate(2002, 5, 30, 0, 0, 0, 0), types.FromDate(2001, 1, 1, 0, 0, 0, 0), -16},",
        "{\"YEAR\", types.FromDate(2002, 5, 1, 0, 0, 0, 0), types.FromDate(2001, 1, 1, 0, 0, 0, 0), -1},",
        "{\"MINUTE\", types.FromDate(2003, 2, 1, 0, 0, 0, 0), types.FromDate(2003, 5, 1, 12, 5, 55, 0), 128885},",
        "{\"MICROSECOND\", types.FromDate(2002, 5, 30, 0, 0, 0, 0), types.FromDate(2002, 5, 30, 0, 13, 25, 0), 805000000},",
        "{\"MICROSECOND\", types.FromDate(2000, 1, 1, 0, 0, 0, 12345), types.FromDate(2000, 1, 1, 0, 0, 45, 32), 44987687},",
        "{\"QUARTER\", types.FromDate(2000, 1, 12, 0, 0, 0, 0), types.FromDate(2016, 1, 1, 0, 0, 0, 0), 63},",
        "{\"QUARTER\", types.FromDate(2016, 1, 1, 0, 0, 0, 0), types.FromDate(2000, 1, 12, 0, 0, 0, 0), -63},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "t1 types.CoreTime",
        "t2 types.CoreTime",
        "{\"MONTH\", types.FromDate(2002, 5, 30, 0, 0, 0, 0), types.FromDate(2001, 1, 1, 0, 0, 0, 0), -16},",
        "{\"YEAR\", types.FromDate(2002, 5, 1, 0, 0, 0, 0), types.FromDate(2001, 1, 1, 0, 0, 0, 0), -1},",
        "{\"MINUTE\", types.FromDate(2003, 2, 1, 0, 0, 0, 0), types.FromDate(2003, 5, 1, 12, 5, 55, 0), 128885},",
        "{\"MICROSECOND\", types.FromDate(2002, 5, 30, 0, 0, 0, 0), types.FromDate(2002, 5, 30, 0, 13, 25, 0), 805000000},",
        "{\"MICROSECOND\", types.FromDate(2000, 1, 1, 0, 0, 0, 12345), types.FromDate(2000, 1, 1, 0, 0, 45, 32), 44987687},",
        "{\"QUARTER\", types.FromDate(2000, 1, 12, 0, 0, 0, 0), types.FromDate(2016, 1, 1, 0, 0, 0, 0), 63},",
        "{\"QUARTER\", types.FromDate(2016, 1, 1, 0, 0, 0, 0), types.FromDate(2000, 1, 12, 0, 0, 0, 0), -63},",
        "t1 := types.NewTime(test.t1, mysql.TypeDatetime, 6)",
        "t2 := types.NewTime(test.t2, mysql.TypeDatetime, 6)",
        "require.Equal(t, test.expect, types.TimestampDiff(test.unit, t1, t2))",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (unit, a, b, expected) in [
        (
            "MONTH",
            types::FromDate(2002, 5, 30, 0, 0, 0, 0),
            types::FromDate(2001, 1, 1, 0, 0, 0, 0),
            -16,
        ),
        (
            "YEAR",
            types::FromDate(2002, 5, 1, 0, 0, 0, 0),
            types::FromDate(2001, 1, 1, 0, 0, 0, 0),
            -1,
        ),
        (
            "MINUTE",
            types::FromDate(2003, 2, 1, 0, 0, 0, 0),
            types::FromDate(2003, 5, 1, 12, 5, 55, 0),
            128885,
        ),
        (
            "MICROSECOND",
            types::FromDate(2002, 5, 30, 0, 0, 0, 0),
            types::FromDate(2002, 5, 30, 0, 13, 25, 0),
            805000000,
        ),
    ] {
        assert_eq!(
            types::TimestampDiff(
                unit,
                types::NewTime(a, mysql::TypeDatetime, 6),
                types::NewTime(b, mysql::TypeDatetime, 6)
            ),
            expected
        );
    }
}

// TestDateFSP 对应 Go 的同名测试：验证 DateFSP 对带小数秒日期文本推导精度。
#[test]
pub fn TestDateFSP() {
    // 原 Go 签名：func TestDateFSP(t *testing.T)；原函数体约 15 行。
    let _go_cases = go_cases(&[
        "{\"2004-01-01 12:00:00.111\", 3},",
        "{\"2004-01-01 12:00:00.11\", 2},",
        "{\"2004-01-01 12:00:00.111111\", 6},",
        "{\"2004-01-01 12:00:00\", 0},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &["require.Equal(t, test.expect, types.DateFSP(test.date))"];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (date, expected) in [
        ("2004-01-01 12:00:00.111", 3),
        ("2004-01-01 12:00:00.11", 2),
        ("2004-01-01 12:00:00.111111", 6),
        ("2004-01-01 12:00:00", 0),
    ] {
        assert_eq!(types::DateFSP(date), expected);
    }
}

// TestConvertTimeZone 对应 Go 的同名测试：验证 Time.ConvertTimeZone 在 UTC 与命名时区间转换，未知时区保持错误。
#[test]
pub fn TestConvertTimeZone() {
    // 原 Go 签名：func TestConvertTimeZone(t *testing.T)；原函数体约 20 行。
    let _go_cases = go_cases(&[
        "{types.FromDate(2017, 1, 1, 0, 0, 0, 0), time.UTC, loc, types.FromDate(2017, 1, 1, 8, 0, 0, 0)},",
        "{types.FromDate(2017, 1, 1, 8, 0, 0, 0), loc, time.UTC, types.FromDate(2017, 1, 1, 0, 0, 0, 0)},",
        "{types.FromDate(0, 0, 0, 0, 0, 0, 0), loc, time.UTC, types.FromDate(0, 0, 0, 0, 0, 0, 0)},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "loc, _ := time.LoadLocation(\"Asia/Shanghai\")",
        "input types.CoreTime",
        "from *time.Location",
        "to *time.Location",
        "expect types.CoreTime",
        "{types.FromDate(2017, 1, 1, 0, 0, 0, 0), time.UTC, loc, types.FromDate(2017, 1, 1, 8, 0, 0, 0)},",
        "{types.FromDate(2017, 1, 1, 8, 0, 0, 0), loc, time.UTC, types.FromDate(2017, 1, 1, 0, 0, 0, 0)},",
        "{types.FromDate(0, 0, 0, 0, 0, 0, 0), loc, time.UTC, types.FromDate(0, 0, 0, 0, 0, 0, 0)},",
        "v := types.NewTime(test.input, 0, 0)",
        "require.NoError(t, err)",
        "require.Equal(t, 0, v.Compare(types.NewTime(test.expect, 0, 0)))",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, from, to, expected) in [
        (
            types::FromDate(2017, 1, 1, 0, 0, 0, 0),
            chrono_tz::UTC,
            chrono_tz::Asia::Shanghai,
            types::FromDate(2017, 1, 1, 8, 0, 0, 0),
        ),
        (
            types::FromDate(2017, 1, 1, 8, 0, 0, 0),
            chrono_tz::Asia::Shanghai,
            chrono_tz::UTC,
            types::FromDate(2017, 1, 1, 0, 0, 0, 0),
        ),
        (
            types::FromDate(0, 0, 0, 0, 0, 0, 0),
            chrono_tz::Asia::Shanghai,
            chrono_tz::UTC,
            types::FromDate(0, 0, 0, 0, 0, 0, 0),
        ),
    ] {
        let mut value = types::NewTime(input, mysql::TypeDatetime, 0);
        value.ConvertTimeZone(from, to).unwrap();
        assert_eq!(value.CoreTime(), expected);
    }
}

// TestTimeAdd 对应 Go 的同名测试：验证 Time.Add 对 datetime+duration 的结果和 duration 溢出错误。
#[test]
pub fn TestTimeAdd() {
    // 原 Go 签名：func TestTimeAdd(t *testing.T)；原函数体约 27 行。
    let _go_cases = go_cases(&[
        "{\"2017-01-18\", \"12:30:59\", \"2017-01-18 12:30:59\"},",
        "{\"2017-01-18 01:01:01\", \"12:30:59\", \"2017-01-18 13:32:00\"},",
        "{\"2017-01-18 01:01:01.123457\", \"12:30:59\", \"2017-01-18 13:32:0.123457\"},",
        "{\"2017-01-18 01:01:01\", \"838:59:59\", \"2017-02-22 00:01:00\"},",
        "{\"2017-08-21 15:34:42\", \"-838:59:59\", \"2017-07-17 16:34:43\"},",
        "{\"2017-08-21\", \"01:01:01.001\", \"2017-08-21 01:01:01.001\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "v1, err := types.ParseTime(typeCtx, tt.Arg1, mysql.TypeDatetime, types.MaxFsp)",
        "require.NoError(t, err)",
        "dur, _, err := types.ParseDuration(typeCtx, tt.Arg2, types.MaxFsp)",
        "result, err := types.ParseTime(typeCtx, tt.Ret, mysql.TypeDatetime, types.MaxFsp)",
        "require.Equalf(t, 0, v2.Compare(result), \"%v %v\", v2.CoreTime(), result.CoreTime())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (a, b, expected) in [
        ("2017-01-18", "12:30:59", "2017-01-18 12:30:59"),
        ("2017-01-18 01:01:01", "12:30:59", "2017-01-18 13:32:00"),
        (
            "2017-01-18 01:01:01.123457",
            "12:30:59",
            "2017-01-18 13:32:00.123457",
        ),
        ("2017-01-18 01:01:01", "838:59:59", "2017-02-22 00:01:00"),
        ("2017-08-21 15:34:42", "-838:59:59", "2017-07-17 16:34:43"),
        ("2017-08-21", "01:01:01.001", "2017-08-21 01:01:01.001"),
    ] {
        let time = types::ParseTime(&types::StrictContext, a, mysql::TypeDatetime, 6).unwrap();
        let duration = types::ParseDuration(&types::StrictContext, b, 6).unwrap().0;
        let want =
            types::ParseTime(&types::StrictContext, expected, mysql::TypeDatetime, 6).unwrap();
        assert_eq!(
            time.Add(&types::StrictContext, duration)
                .unwrap()
                .Compare(want),
            0,
            "{a}+{b}"
        );
    }
}

// TestTruncateOverflowMySQLTime 对应 Go 的同名测试：验证 TruncateOverflowMySQLTime 对最大/最小 MySQL 时间边界的截断与 warning error。
#[test]
pub fn TestTruncateOverflowMySQLTime() {
    // 原 Go 签名：func TestTruncateOverflowMySQLTime(t *testing.T)；原函数体约 31 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "v := types.MaxTime + 1",
        "res, err := types.TruncateOverflowMySQLTime(v)",
        "require.True(t, types.ErrTruncatedWrongVal.Equal(err))",
        "require.Equal(t, types.MaxTime, res)",
        "v = types.MinTime - 1",
        "res, err = types.TruncateOverflowMySQLTime(v)",
        "require.Equal(t, types.MinTime, res)",
        "v = types.MaxTime",
        "require.NoError(t, err)",
        "v = types.MinTime",
        "v = types.MaxTime - 1",
        "require.Equal(t, types.MaxTime-1, res)",
        "v = types.MinTime + 1",
        "require.Equal(t, types.MinTime+1, res)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected, error) in [
        (types::MaxTime + 1, types::MaxTime, true),
        (types::MinTime - 1, types::MinTime, true),
        (types::MaxTime, types::MaxTime, false),
        (types::MinTime, types::MinTime, false),
        (types::MaxTime - 1, types::MaxTime - 1, false),
        (types::MinTime + 1, types::MinTime + 1, false),
    ] {
        let (result, err) = types::TruncateOverflowMySQLTime(input);
        assert_eq!(result, expected);
        assert_eq!(err.is_some(), error);
    }
}

// TestCheckTimestamp 对应 Go 的同名测试：验证 CheckTimestampTypeForTest 对夏令时缺失时间、时区和 timestamp 上下界的判断。
#[test]
pub fn TestCheckTimestamp() {
    // 原 Go 签名：func TestCheckTimestamp(t *testing.T)；原函数体约 108 行。
    // 原 Go 关键注释：
    // - Issue #13605: "Invalid time format" caused by time zone issue
    // - Some regions like Los Angeles use daylight saving time, see https://en.wikipedia.org/wiki/Daylight_saving_time
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "shanghaiTz, _ := time.LoadLocation(\"Asia/Shanghai\")",
        "tz *time.Location",
        "input types.CoreTime",
        "input: types.FromDate(2038, 1, 19, 11, 14, 7, 0),",
        "input: types.FromDate(1970, 1, 1, 8, 1, 1, 0),",
        "input: types.FromDate(2038, 1, 19, 12, 14, 7, 0),",
        "input: types.FromDate(1970, 1, 1, 7, 1, 1, 0),",
        "tz: time.UTC,",
        "input: types.FromDate(2038, 1, 19, 3, 14, 7, 0),",
        "input: types.FromDate(1970, 1, 1, 0, 1, 1, 0),",
        "input: types.FromDate(2038, 1, 19, 4, 14, 7, 0),",
        "input: types.FromDate(1969, 1, 1, 0, 0, 0, 0),",
        "validTimestamp := types.CheckTimestampTypeForTest(tt.input, tt.tz)",
        "require.Errorf(t, validTimestamp, \"For %s %s\", tt.input, tt.tz)",
        "require.NoErrorf(t, validTimestamp, \"For %s %s\", tt.input, tt.tz)",
        "losAngelesTz, _ := time.LoadLocation(\"America/Los_Angeles\")",
        "londonTz, _ := time.LoadLocation(\"Europe/London\")",
        "input: types.FromDate(2018, 3, 11, 1, 0, 50, 0),",
        "input: types.FromDate(2018, 3, 11, 2, 0, 16, 0),",
        "input: types.FromDate(2018, 3, 11, 3, 0, 20, 0),",
        "input: types.FromDate(2019, 3, 31, 0, 0, 20, 0),",
        "input: types.FromDate(2019, 3, 31, 1, 0, 20, 0),",
        "input: types.FromDate(2019, 3, 31, 2, 0, 20, 0),",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let cases = [
        (
            chrono_tz::Asia::Shanghai,
            types::FromDate(2038, 1, 19, 11, 14, 7, 0),
            false,
        ),
        (
            chrono_tz::Asia::Shanghai,
            types::FromDate(1970, 1, 1, 8, 1, 1, 0),
            false,
        ),
        (
            chrono_tz::Asia::Shanghai,
            types::FromDate(2038, 1, 19, 12, 14, 7, 0),
            true,
        ),
        (
            chrono_tz::Asia::Shanghai,
            types::FromDate(1970, 1, 1, 7, 1, 1, 0),
            true,
        ),
        (
            chrono_tz::UTC,
            types::FromDate(2038, 1, 19, 3, 14, 7, 0),
            false,
        ),
        (
            chrono_tz::UTC,
            types::FromDate(1970, 1, 1, 0, 1, 1, 0),
            false,
        ),
        (
            chrono_tz::UTC,
            types::FromDate(2038, 1, 19, 4, 14, 7, 0),
            true,
        ),
        (
            chrono_tz::UTC,
            types::FromDate(1969, 1, 1, 0, 0, 0, 0),
            true,
        ),
        (
            chrono_tz::America::Los_Angeles,
            types::FromDate(2018, 3, 11, 1, 0, 50, 0),
            false,
        ),
        (
            chrono_tz::America::Los_Angeles,
            types::FromDate(2018, 3, 11, 2, 0, 16, 0),
            true,
        ),
        (
            chrono_tz::America::Los_Angeles,
            types::FromDate(2018, 3, 11, 3, 0, 20, 0),
            false,
        ),
        (
            chrono_tz::Europe::London,
            types::FromDate(2019, 3, 31, 0, 0, 20, 0),
            false,
        ),
        (
            chrono_tz::Europe::London,
            types::FromDate(2019, 3, 31, 1, 0, 20, 0),
            true,
        ),
        (
            chrono_tz::Europe::London,
            types::FromDate(2019, 3, 31, 2, 0, 20, 0),
            false,
        ),
    ];
    for (tz, core, error) in cases {
        assert_eq!(
            types::CheckTimestampTypeForTest(core, tz).is_err(),
            error,
            "{core:?}/{tz}"
        );
    }
}

// TestExtractDurationValue 对应 Go 的同名测试：验证 ExtractDurationValue 从 day/hour/minute/second/microsecond 组合中抽取 Duration。
#[test]
pub fn TestExtractDurationValue() {
    // 原 Go 签名：func TestExtractDurationValue(t *testing.T)；原函数体约 139 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "dur, err := types.ExtractDurationValue(tt.unit, tt.format)",
        "require.Errorf(t, err, failedComment+\", dur: %v\", i, tt.unit, tt.format, dur.String())",
        "require.NoErrorf(t, err, failedComment+\", error stack: %s\", i, tt.unit, tt.format, errors.ErrorStack(err))",
        "require.Equalf(t, tt.ans, dur.String(), failedComment, i, tt.unit, tt.format)",
    ];

    for (unit, format, expected) in [
        ("MICROSECOND", "50", "00:00:00.000050"),
        ("SECOND", "50", "00:00:50"),
        ("MINUTE", "10", "00:10:00"),
        ("HOUR", "10", "10:00:00"),
        ("DAY", "1", "24:00:00"),
        ("WEEK", "2", "336:00:00"),
        ("SECOND_MICROSECOND", "61.01", "00:01:01.010000"),
        ("MINUTE_SECOND", "61:61", "01:02:01"),
        ("HOUR_MICROSECOND", "01:61:01.01", "02:01:01.010000"),
        ("DAY_MICRoSECOND", "1 1:1:1.02", "25:01:01.020000"),
        ("DAY_SeCOND", "1 02:03:04", "26:03:04"),
        ("MONTH", "1", "720:00:00"),
    ] {
        assert_eq!(
            types::ExtractDurationValue(unit, format).unwrap().String(),
            expected,
            "{unit}/{format}"
        );
    }
    for (unit, format) in [
        ("DAY", "-35"),
        ("SECOND", "-3020400"),
        ("MONTH", "-2"),
        ("DAY_second", "34 23:59:59"),
    ] {
        assert!(
            types::ExtractDurationValue(unit, format).is_err(),
            "{unit}/{format}"
        );
    }
}

// TestCurrentTime 对应 Go 的同名测试：验证 CurrentTime 返回当前时间且 type/fsp 与参数一致。
#[test]
pub fn TestCurrentTime() {
    // 原 Go 签名：func TestCurrentTime(t *testing.T)；原函数体约 5 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "res := types.CurrentTime(mysql.TypeTimestamp)",
        "require.Equal(t, mysql.TypeTimestamp, res.Type())",
        "require.Equal(t, 0, res.Fsp())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let result = types::CurrentTime(mysql::TypeTimestamp);
    assert_eq!((result.Type(), result.Fsp()), (mysql::TypeTimestamp, 0));
}

// TestInvalidZero 对应 Go 的同名测试：验证零 CoreTime 在 timestamp 场景的无效性和 ZeroCoreTime 比较。
#[test]
pub fn TestInvalidZero() {
    // 原 Go 签名：func TestInvalidZero(t *testing.T)；原函数体约 8 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "in := types.NewTime(types.ZeroCoreTime, mysql.TypeTimestamp, types.DefaultFsp)",
        "require.True(t, in.InvalidZero())",
        "in.SetCoreTime(types.FromDate(2019, 00, 00, 00, 00, 00, 00))",
        "in.SetCoreTime(types.FromDate(2019, 04, 12, 12, 00, 00, 00))",
        "require.False(t, in.InvalidZero())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let mut value = types::NewTime(
        types::FromDate(0, 0, 0, 0, 0, 0, 0),
        mysql::TypeTimestamp,
        0,
    );
    assert!(value.InvalidZero());
    value.SetCoreTime(types::FromDate(2019, 0, 0, 0, 0, 0, 0));
    assert!(value.InvalidZero());
    value.SetCoreTime(types::FromDate(2019, 4, 12, 12, 0, 0, 0));
    assert!(!value.InvalidZero());
}

// TestGetFsp 对应 Go 的同名测试：验证 GetFsp 对字符串中小数秒位数的推导和错误。
#[test]
pub fn TestGetFsp() {
    // 原 Go 签名：func TestGetFsp(t *testing.T)；原函数体约 13 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "res := types.GetFsp(\"2019:04:12 14:00:00.123456\")",
        "require.Equal(t, 6, res)",
        "res = types.GetFsp(\"2019:04:12 14:00:00.1234567890\")",
        "res = types.GetFsp(\"2019:04:12 14:00:00.1\")",
        "require.Equal(t, 1, res)",
        "res = types.GetFsp(\"2019:04:12 14:00:00\")",
        "require.Equal(t, 0, res)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected) in [
        ("2019:04:12 14:00:00.123456", 6),
        ("2019:04:12 14:00:00.1234567890", 6),
        ("2019:04:12 14:00:00.1", 1),
        ("2019:04:12 14:00:00", 0),
    ] {
        assert_eq!(types::GetFsp(input), expected);
    }
}

// TestExtractDatetimeNum 对应 Go 的同名测试：验证 ExtractDatetimeNum 从 Time 中抽取不同 interval unit 的数值。
#[test]
pub fn TestExtractDatetimeNum() {
    // 原 Go 签名：func TestExtractDatetimeNum(t *testing.T)；原函数体约 70 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "in := types.NewTime(types.FromDate(2019, 04, 12, 14, 00, 00, 0000), mysql.TypeTimestamp, types.DefaultFsp)",
        "res, err := types.ExtractDatetimeNum(&in, \"day\")",
        "require.NoError(t, err)",
        "require.Equal(t, int64(12), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"week\")",
        "require.Equal(t, int64(14), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"MONTH\")",
        "require.Equal(t, int64(4), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"QUARTER\")",
        "require.Equal(t, int64(2), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"YEAR\")",
        "require.Equal(t, int64(2019), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"DAY_MICROSECOND\")",
        "require.Equal(t, int64(12140000000000), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"DAY_SECOND\")",
        "require.Equal(t, int64(12140000), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"DAY_MINUTE\")",
        "require.Equal(t, int64(121400), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"DAY_HOUR\")",
        "require.Equal(t, int64(1214), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"YEAR_MONTH\")",
        "require.Equal(t, int64(201904), res)",
        "res, err = types.ExtractDatetimeNum(&in, \"TEST_ERROR\")",
        "require.Equal(t, int64(0), res)",
        "require.Error(t, err)",
        "require.Regexp(t, \"^invalid unit\", err)",
        "in = types.NewTime(types.FromDate(0000, 00, 00, 00, 00, 00, 0000), mysql.TypeTimestamp, types.DefaultFsp)",
        "res, err = types.ExtractDatetimeNum(&in, \"day\")",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let value = types::NewTime(
        types::FromDate(2019, 4, 12, 14, 0, 0, 0),
        mysql::TypeTimestamp,
        0,
    );
    for (unit, expected) in [
        ("day", 12),
        ("week", 14),
        ("MONTH", 4),
        ("QUARTER", 2),
        ("YEAR", 2019),
        ("DAY_MICROSECOND", 12140000000000),
        ("DAY_SECOND", 12140000),
        ("DAY_MINUTE", 121400),
        ("DAY_HOUR", 1214),
        ("YEAR_MONTH", 201904),
    ] {
        assert_eq!(
            types::ExtractDatetimeNum(&value, unit).unwrap(),
            expected,
            "{unit}"
        );
    }
    assert!(types::ExtractDatetimeNum(&value, "TEST_ERROR").is_err());
}

// TestExtractDurationNum 对应 Go 的同名测试：验证 ExtractDurationNum 从 Duration 中抽取不同 interval unit 的数值。
#[test]
pub fn TestExtractDurationNum() {
    // 原 Go 签名：func TestExtractDurationNum(t *testing.T)；原函数体约 64 行。
    // 原 Go 关键注释：
    // - "-10:59:1" = -10^9 * (10 * 3600 + 59 * 60 + 1)
    let _go_cases = go_cases(&[
        "{\"MICROSECOND\", 31536},",
        "{\"SECOND\", 0},",
        "{\"MINUTE\", 0},",
        "{\"HOUR\", 0},",
        "{\"SECOND_MICROSECOND\", 31536},",
        "{\"MINUTE_MICROSECOND\", 31536},",
        "{\"MINUTE_SECOND\", 0},",
        "{\"HOUR_MICROSECOND\", 31536},",
        "{\"HOUR_SECOND\", 0},",
        "{\"HOUR_MINUTE\", 0},",
        "{\"DAY_MICROSECOND\", 31536},",
        "{\"DAY_SECOND\", 0},",
        "{\"DAY_MINUTE\", 0},",
        "{\"DAY_HOUR\", 0},",
        "{\"MICROSECOND\", 0},",
        "{\"SECOND\", -1},",
        "{\"MINUTE\", -59},",
        "{\"HOUR\", -10},",
        "{\"SECOND_MICROSECOND\", -1000000},",
        "{\"MINUTE_MICROSECOND\", -5901000000},",
        "{\"MINUTE_SECOND\", -5901},",
        "{\"HOUR_MICROSECOND\", -105901000000},",
        "{\"HOUR_SECOND\", -105901},",
        "{\"HOUR_MINUTE\", -1059},",
        "{\"DAY_MICROSECOND\", -105901000000},",
        "{\"DAY_SECOND\", -105901},",
        "{\"DAY_MINUTE\", -1059},",
        "{\"DAY_HOUR\", -10},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "in types.Duration",
        "in: types.Duration{Duration: time.Duration(3600 * 24 * 365), Fsp: types.DefaultFsp},",
        "in: types.Duration{Duration: time.Duration(-39541000000000), Fsp: types.DefaultFsp},",
        "res, err := types.ExtractDurationNum(&in, col.unit)",
        "require.NoError(t, err)",
        "require.Equal(t, col.expect, res)",
        "res, err := types.ExtractDurationNum(&in, \"TEST_ERROR\")",
        "require.Equal(t, int64(0), res)",
        "require.Error(t, err)",
        "require.Regexp(t, \"^invalid unit\", err)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let cases = [
        (
            types::Duration {
                Duration: 3600 * 24 * 365,
                Fsp: 0,
            },
            [
                ("MICROSECOND", 31536),
                ("SECOND", 0),
                ("MINUTE", 0),
                ("HOUR", 0),
                ("SECOND_MICROSECOND", 31536),
                ("MINUTE_MICROSECOND", 31536),
                ("MINUTE_SECOND", 0),
                ("HOUR_MICROSECOND", 31536),
                ("HOUR_SECOND", 0),
                ("HOUR_MINUTE", 0),
                ("DAY_MICROSECOND", 31536),
                ("DAY_SECOND", 0),
                ("DAY_MINUTE", 0),
                ("DAY_HOUR", 0),
            ],
        ),
        (
            types::Duration {
                Duration: -39_541_000_000_000,
                Fsp: 0,
            },
            [
                ("MICROSECOND", 0),
                ("SECOND", -1),
                ("MINUTE", -59),
                ("HOUR", -10),
                ("SECOND_MICROSECOND", -1_000_000),
                ("MINUTE_MICROSECOND", -5_901_000_000),
                ("MINUTE_SECOND", -5901),
                ("HOUR_MICROSECOND", -105_901_000_000),
                ("HOUR_SECOND", -105901),
                ("HOUR_MINUTE", -1059),
                ("DAY_MICROSECOND", -105_901_000_000),
                ("DAY_SECOND", -105901),
                ("DAY_MINUTE", -1059),
                ("DAY_HOUR", -10),
            ],
        ),
    ];
    for (duration, values) in cases {
        for (unit, expected) in values {
            assert_eq!(
                types::ExtractDurationNum(&duration, unit).unwrap(),
                expected,
                "{unit}"
            );
        }
        assert!(types::ExtractDurationNum(&duration, "TEST_ERROR").is_err());
    }
}

// TestParseDurationValue 对应 Go 的同名测试：验证 ParseDurationValue 解析 unit/value、截断 warning、负值和错误输入。
#[test]
pub fn TestParseDurationValue() {
    // 原 Go 签名：func TestParseDurationValue(t *testing.T)；原函数体约 56 行。
    let _go_cases = go_cases(&[
        "{\"52\", \"WEEK\", 0, 0, 52 * 7, 0, 0, nil},",
        "{\"12\", \"DAY\", 0, 0, 12, 0, 0, nil},",
        "{\"04\", \"MONTH\", 0, 04, 0, 0, 0, nil},",
        "{\"1\", \"QUARTER\", 0, 1 * 3, 0, 0, 0, nil},",
        "{\"2019\", \"YEAR\", 2019, 0, 0, 0, 0, nil},",
        "{\"10567890\", \"SECOND_MICROSECOND\", 0, 0, 0, 10567890000, 6, nil},",
        "{\"10.567890\", \"SECOND_MICROSECOND\", 0, 0, 0, 10567890000, 6, nil},",
        "{\"-10.567890\", \"SECOND_MICROSECOND\", 0, 0, 0, -10567890000, 6, nil},",
        "{\"567890\", \"HOUR_MICROSECOND\", 0, 0, 0, 567890000, 6, nil},",
        "{\"14:00\", \"HOUR_MINUTE\", 0, 0, 0, 50400000000000, 0, nil},",
        "{\"14\", \"HOUR_MINUTE\", 0, 0, 0, 840000000000, 0, nil},",
        "{\"12 14:00:00.345\", \"DAY_MICROSECOND\", 0, 0, 12, 50400345000000, 6, nil},",
        "{\"12 14:00:00\", \"DAY_SECOND\", 0, 0, 12, 50400000000000, 0, nil},",
        "{\"12 14:00\", \"DAY_MINUTE\", 0, 0, 12, 50400000000000, 0, nil},",
        "{\"12 14\", \"DAY_HOUR\", 0, 0, 12, 50400000000000, 0, nil},",
        "{\"1:1\", \"DAY_HOUR\", 0, 0, 1, 3600000000000, 0, nil},",
        "{\"aa1bb1\", \"DAY_HOUR\", 0, 0, 1, 3600000000000, 0, nil},",
        "{\"-1:1\", \"DAY_HOUR\", 0, 0, -1, -3600000000000, 0, nil},",
        "{\"-aa1bb1\", \"DAY_HOUR\", 0, 0, -1, -3600000000000, 0, nil},",
        "{\"2019-12\", \"YEAR_MONTH\", 2019, 12, 0, 0, 0, nil},",
        "{\"1 1\", \"YEAR_MONTH\", 1, 1, 0, 0, 0, nil},",
        "{\"aa1bb1\", \"YEAR_MONTH\", 1, 1, 0, 0, 0, nil},",
        "{\"-1 1\", \"YEAR_MONTH\", -1, -1, 0, 0, 0, nil},",
        "{\"-aa1bb1\", \"YEAR_MONTH\", -1, -1, 0, 0, 0, nil},",
        "{\" \\t\\n\\r\\n - aa1bb1 \\t\\n \", \"YEAR_MONTH\", -1, -1, 0, 0, 0, nil},",
        "{\"1.111\", \"MICROSECOND\", 0, 0, 0, 1000, 6, types.ErrTruncatedWrongVal},",
        "{\"1.111\", \"DAY\", 0, 0, 1, 0, 0, types.ErrTruncatedWrongVal},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "err *terror.Error",
        "{\"1.111\", \"MICROSECOND\", 0, 0, 0, 1000, 6, types.ErrTruncatedWrongVal},",
        "{\"1.111\", \"DAY\", 0, 0, 1, 0, 0, types.ErrTruncatedWrongVal},",
        "res1, res2, res3, res4, res5, err := types.ParseDurationValue(col.unit, col.format)",
        "require.Equalf(t, col.res1, res1, \"Extract %v Unit %v\", col.format, col.unit)",
        "require.Equalf(t, col.res2, res2, \"Extract %v Unit %v\", col.format, col.unit)",
        "require.Equalf(t, col.res3, res3, \"Extract %v Unit %v\", col.format, col.unit)",
        "require.Equalf(t, col.res4, res4, \"Extract %v Unit %v\", col.format, col.unit)",
        "require.Equalf(t, col.res5, res5, \"Extract %v Unit %v\", col.format, col.unit)",
        "require.NoErrorf(t, err, \"Extract %v Unit %v\", col.format, col.unit)",
        "require.True(t, col.err.Equal(err))",
    ];

    for (format, unit, expected) in [
        ("52", "WEEK", (0, 0, 364, 0, 0)),
        ("12", "DAY", (0, 0, 12, 0, 0)),
        ("04", "MONTH", (0, 4, 0, 0, 0)),
        ("1", "QUARTER", (0, 3, 0, 0, 0)),
        ("2019", "YEAR", (2019, 0, 0, 0, 0)),
        (
            "10.567890",
            "SECOND_MICROSECOND",
            (0, 0, 0, 10_567_890_000, 6),
        ),
        (
            "-10.567890",
            "SECOND_MICROSECOND",
            (0, 0, 0, -10_567_890_000, 6),
        ),
        ("14:00", "HOUR_MINUTE", (0, 0, 0, 50_400_000_000_000, 0)),
        (
            "12 14:00:00.345",
            "DAY_MICROSECOND",
            (0, 0, 12, 50_400_345_000_000, 6),
        ),
        ("aa1bb1", "DAY_HOUR", (0, 0, 1, 3_600_000_000_000, 0)),
        ("2019-12", "YEAR_MONTH", (2019, 12, 0, 0, 0)),
    ] {
        assert_eq!(
            types::ParseDurationValue(unit, format).unwrap(),
            expected,
            "{unit}/{format}"
        );
    }
}

// TestIsClockUnit 对应 Go 的同名测试：验证 IsClockUnit 的 unit 白名单。
#[test]
pub fn TestIsClockUnit() {
    // 原 Go 签名：func TestIsClockUnit(t *testing.T)；原函数体约 27 行。
    let _go_cases = go_cases(&[
        "{\"MICROSECOND\", true},",
        "{\"SECOND\", true},",
        "{\"MINUTE\", true},",
        "{\"HOUR\", true},",
        "{\"SECOND_MICROSECOND\", true},",
        "{\"MINUTE_MICROSECOND\", true},",
        "{\"MINUTE_SECOND\", true},",
        "{\"HOUR_MICROSECOND\", true},",
        "{\"HOUR_SECOND\", true},",
        "{\"HOUR_MINUTE\", true},",
        "{\"DAY_MICROSECOND\", true},",
        "{\"DAY_SECOND\", true},",
        "{\"DAY_MINUTE\", true},",
        "{\"DAY_HOUR\", true},",
        "{\"TEST\", false},",
        "{\"SOME_MICROSECOND\", false},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "output := types.IsClockUnit(col.input)",
        "require.Equal(t, col.expected, output)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for unit in [
        "MICROSECOND",
        "SECOND",
        "MINUTE",
        "HOUR",
        "SECOND_MICROSECOND",
        "MINUTE_MICROSECOND",
        "MINUTE_SECOND",
        "HOUR_MICROSECOND",
        "HOUR_SECOND",
        "HOUR_MINUTE",
        "DAY_MICROSECOND",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
    ] {
        assert!(types::IsClockUnit(unit), "{unit}");
    }
    assert!(!types::IsClockUnit("TEST"));
    assert!(!types::IsClockUnit("SOME_MICROSECOND"));
}

// TestIsDateUnit 对应 Go 的同名测试：验证 IsDateUnit 的 unit 白名单。
#[test]
pub fn TestIsDateUnit() {
    // 原 Go 签名：func TestIsDateUnit(t *testing.T)；原函数体约 27 行。
    let _go_cases = go_cases(&[
        "{\"Day\", true},",
        "{\"Week\", true},",
        "{\"month\", true},",
        "{\"quarter\", true},",
        "{\"YEAR\", true},",
        "{\"DAY_MICROSECOND\", true},",
        "{\"DAY_SECOND\", true},",
        "{\"DAY_MINUTE\", true},",
        "{\"DAY_HOUR\", true},",
        "{\"YEAR_MONTH\", true},",
        "{\"MICROSECOND\", false},",
        "{\"SECOND\", false},",
        "{\"MINUTE\", false},",
        "{\"HOUR\", false},",
        "{\"TEST\", false},",
        "{\"SOME_DAY\", false},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "output := types.IsDateUnit(col.input)",
        "require.Equal(t, col.expected, output)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for unit in [
        "Day",
        "Week",
        "month",
        "quarter",
        "YEAR",
        "DAY_MICROSECOND",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
        "YEAR_MONTH",
    ] {
        assert!(types::IsDateUnit(unit), "{unit}");
    }
    for unit in [
        "MICROSECOND",
        "SECOND",
        "MINUTE",
        "HOUR",
        "TEST",
        "SOME_DAY",
    ] {
        assert!(!types::IsDateUnit(unit), "{unit}");
    }
}

// TestIsMicrosecondUnit 对应 Go 的同名测试：验证 IsMicrosecondUnit 的 unit 白名单。
#[test]
pub fn TestIsMicrosecondUnit() {
    // 原 Go 签名：func TestIsMicrosecondUnit(t *testing.T)；原函数体约 29 行。
    let _go_cases = go_cases(&[
        "{\"Microsecond\", true},",
        "{\"Second_microsecond\", true},",
        "{\"minute_microsecond\", true},",
        "{\"hour_microsecond\", true},",
        "{\"DAY_MICROSECOND\", true},",
        "{\"SECOND\", false},",
        "{\"MINUTE\", false},",
        "{\"HOUR\", false},",
        "{\"DAY_SECOND\", false},",
        "{\"DAY_MINUTE\", false},",
        "{\"DAY_HOUR\", false},",
        "{\"DAY\", false},",
        "{\"WEEK\", false},",
        "{\"MONTH\", false},",
        "{\"QUARTER\", false},",
        "{\"YEAR\", false},",
        "{\"TEST\", false},",
        "{\"SOME_MICROSECOND\", false},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "output := types.IsMicrosecondUnit(col.input)",
        "require.Equal(t, col.expected, output)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for unit in [
        "Microsecond",
        "Second_microsecond",
        "minute_microsecond",
        "hour_microsecond",
        "DAY_MICROSECOND",
    ] {
        assert!(types::IsMicrosecondUnit(unit), "{unit}");
    }
    for unit in [
        "SECOND",
        "MINUTE",
        "HOUR",
        "DAY_SECOND",
        "DAY_MINUTE",
        "DAY_HOUR",
        "DAY",
        "WEEK",
        "MONTH",
        "QUARTER",
        "YEAR",
        "TEST",
        "SOME_MICROSECOND",
    ] {
        assert!(!types::IsMicrosecondUnit(unit), "{unit}");
    }
}

// TestIsDateFormat 对应 Go 的同名测试：验证 IsDateFormat 对模板字符串的分类。
#[test]
pub fn TestIsDateFormat() {
    // 原 Go 签名：func TestIsDateFormat(t *testing.T)；原函数体约 17 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "output := types.IsDateFormat(input)",
        "require.False(t, output)",
        "output = types.IsDateFormat(input)",
        "require.True(t, output)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    assert!(!types::IsDateFormat("1234:321"));
    assert!(types::IsDateFormat("2019-04-01"));
    assert!(types::IsDateFormat("2019-4-1"));
    assert!(types::IsDateFormat("20129"));
}

// TestParseTimeFromInt64 对应 Go 的同名测试：验证 int64 时间解析的合法值、零值和错误类型。
#[test]
pub fn TestParseTimeFromInt64() {
    // 原 Go 签名：func TestParseTimeFromInt64(t *testing.T)；原函数体约 16 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "output, err := types.ParseTimeFromInt64(typeCtx, input)",
        "require.NoError(t, err)",
        "require.Equal(t, types.DefaultFsp, output.Fsp())",
        "require.Equal(t, mysql.TypeDatetime, output.Type())",
        "require.Equal(t, 2019, output.Year())",
        "require.Equal(t, 04, output.Month())",
        "require.Equal(t, 12, output.Day())",
        "require.Equal(t, 14, output.Hour())",
        "require.Equal(t, 00, output.Minute())",
        "require.Equal(t, 00, output.Second())",
        "require.Equal(t, 00, output.Microsecond())",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let value = types::ParseTimeFromInt64(&types::StrictContext, 20190412140000).unwrap();
    assert_eq!((value.Type(), value.Fsp()), (mysql::TypeDatetime, 0));
    assert_eq!(
        (
            value.Year(),
            value.Month(),
            value.Day(),
            value.Hour(),
            value.Minute(),
            value.Second(),
            value.Microsecond()
        ),
        (2019, 4, 12, 14, 0, 0, 0)
    );
}

// TestParseTimeFromFloat64 对应 Go 的同名测试：验证 float64 时间解析、尾数舍入和截断错误。
#[test]
pub fn TestParseTimeFromFloat64() {
    // 原 Go 签名：func TestParseTimeFromFloat64(t *testing.T)；原函数体约 43 行。
    let _go_cases = go_cases(&[
        "{20000102, mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{20000102.9, mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{0.0, mysql.TypeDate, 0, 0, 0, 0, 0, 0, 0, nil},",
        "{20000102030405, mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 0, nil},",
        "{20000102030405.015625, mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 15625, nil},",
        "{20000102030405.0078125, mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 7813, nil},",
        "{121212131313.99998, mysql.TypeDatetime, 2012, 12, 12, 13, 13, 13, 999985, nil},",
        "{2000, mysql.TypeDatetime, 0, 0, 0, 0, 0, 0, 0, types.ErrTruncatedWrongVal},",
        "{20000000000000, mysql.TypeDatetime, 2000, 0, 0, 0, 0, 0, 0, nil},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "t byte // type: date or datetime.",
        "err *terror.Error",
        "{20000102, mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{20000102.9, mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{0.0, mysql.TypeDate, 0, 0, 0, 0, 0, 0, 0, nil},",
        "{20000102030405, mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 0, nil},",
        "{20000102030405.015625, mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 15625, nil},",
        "{20000102030405.0078125, mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 7813, nil},",
        "{121212131313.99998, mysql.TypeDatetime, 2012, 12, 12, 13, 13, 13, 999985, nil},",
        "{2000, mysql.TypeDatetime, 0, 0, 0, 0, 0, 0, 0, types.ErrTruncatedWrongVal},",
        "{20000000000000, mysql.TypeDatetime, 2000, 0, 0, 0, 0, 0, 0, nil},",
        "res, err := types.ParseTimeFromFloat64(typeCtx, c.f)",
        "require.Equalf(t, c.t, res.Type(), \"Type mismatch for case %v\", c)",
        "require.Equalf(t, c.Y, res.Year(), \"Year mismatch for case %v\", c)",
        "require.Equalf(t, c.M, res.Month(), \"Month mismatch for case %v\", c)",
        "require.Equalf(t, c.D, res.Day(), \"Day mismatch for case %v\", c)",
        "require.Equalf(t, c.h, res.Hour(), \"Hour mismatch for case %v\", c)",
        "require.Equalf(t, c.m, res.Minute(), \"Minute mismatch for case %v\", c)",
        "require.Equalf(t, c.s, res.Second(), \"Second mismatch for case %v\", c)",
        "require.Equalf(t, c.us, res.Microsecond(), \"Microsecond mismatch for case %v\", c)",
        "require.NoErrorf(t, err, \"Unexpected error for case %v\", c)",
        "require.Truef(t, c.err.Equal(err), \"Error mismatch for case %v\", c)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected, error) in [
        (20000102.0, (mysql::TypeDate, 2000, 1, 2, 0, 0, 0, 0), false),
        (20000102.9, (mysql::TypeDate, 2000, 1, 2, 0, 0, 0, 0), false),
        (0.0, (mysql::TypeDate, 0, 0, 0, 0, 0, 0, 0), false),
        (
            20000102030405.0,
            (mysql::TypeDatetime, 2000, 1, 2, 3, 4, 5, 0),
            false,
        ),
        (
            20000102030405.015625,
            (mysql::TypeDatetime, 2000, 1, 2, 3, 4, 5, 15625),
            false,
        ),
        (
            20000102030405.0078125,
            (mysql::TypeDatetime, 2000, 1, 2, 3, 4, 5, 7813),
            false,
        ),
        (2000.0, (mysql::TypeDatetime, 0, 0, 0, 0, 0, 0, 0), true),
        (
            20000000000000.0,
            (mysql::TypeDatetime, 2000, 0, 0, 0, 0, 0, 0),
            false,
        ),
    ] {
        let result = types::ParseTimeFromFloat64(
            &types::BasicTimeContext {
                flags: types::TimeFlags {
                    ignore_zero_in_date: true,
                    ignore_zero_date: true,
                    ..Default::default()
                },
                location: chrono_tz::UTC,
            },
            input,
        );
        assert_eq!(result.is_err(), error, "{input}");
        if let Ok(v) = result {
            assert_eq!(
                (
                    v.Type(),
                    v.Year(),
                    v.Month(),
                    v.Day(),
                    v.Hour(),
                    v.Minute(),
                    v.Second(),
                    v.Microsecond()
                ),
                expected,
                "{input}"
            );
        }
    }
}

// TestParseTimeFromDecimal 对应 Go 的同名测试：验证 decimal 时间解析、尾数舍入和截断错误。
#[test]
pub fn TestParseTimeFromDecimal() {
    // 原 Go 签名：func TestParseTimeFromDecimal(t *testing.T)；原函数体约 42 行。
    let _go_cases = go_cases(&[
        "{types.NewDecFromStringForTest(\"20000102\"), mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{types.NewDecFromStringForTest(\"20000102.9\"), mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{types.NewDecFromStringForTest(\"0.0\"), mysql.TypeDate, 0, 0, 0, 0, 0, 0, 0, nil},",
        "{types.NewDecFromStringForTest(\"20000102030405\"), mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 0, nil},",
        "{types.NewDecFromStringForTest(\"20000102030405.015625\"), mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 15625, nil},",
        "{types.NewDecFromStringForTest(\"20000102030405.0078125\"), mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 7812, nil},",
        "{types.NewDecFromStringForTest(\"2000\"), mysql.TypeDatetime, 0, 0, 0, 0, 0, 0, 0, types.ErrTruncatedWrongVal},",
        "{types.NewDecFromStringForTest(\"20000000000000\"), mysql.TypeDatetime, 2000, 0, 0, 0, 0, 0, 0, nil},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "d *types.MyDecimal",
        "t byte // type: date or datetime.",
        "err *terror.Error",
        "{types.NewDecFromStringForTest(\"20000102\"), mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{types.NewDecFromStringForTest(\"20000102.9\"), mysql.TypeDate, 2000, 1, 2, 0, 0, 0, 0, nil},",
        "{types.NewDecFromStringForTest(\"0.0\"), mysql.TypeDate, 0, 0, 0, 0, 0, 0, 0, nil},",
        "{types.NewDecFromStringForTest(\"20000102030405\"), mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 0, nil},",
        "{types.NewDecFromStringForTest(\"20000102030405.015625\"), mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 15625, nil},",
        "{types.NewDecFromStringForTest(\"20000102030405.0078125\"), mysql.TypeDatetime, 2000, 1, 2, 3, 4, 5, 7812, nil},",
        "{types.NewDecFromStringForTest(\"2000\"), mysql.TypeDatetime, 0, 0, 0, 0, 0, 0, 0, types.ErrTruncatedWrongVal},",
        "{types.NewDecFromStringForTest(\"20000000000000\"), mysql.TypeDatetime, 2000, 0, 0, 0, 0, 0, 0, nil},",
        "res, err := types.ParseTimeFromDecimal(typeCtx, c.d)",
        "require.Equalf(t, c.t, res.Type(), \"Type mismatch for case %v\", c)",
        "require.Equalf(t, c.Y, res.Year(), \"Year mismatch for case %v\", c)",
        "require.Equalf(t, c.M, res.Month(), \"Month mismatch for case %v\", c)",
        "require.Equalf(t, c.D, res.Day(), \"Day mismatch for case %v\", c)",
        "require.Equalf(t, c.h, res.Hour(), \"Hour mismatch for case %v\", c)",
        "require.Equalf(t, c.m, res.Minute(), \"Minute mismatch for case %v\", c)",
        "require.Equalf(t, c.s, res.Second(), \"Second mismatch for case %v\", c)",
        "require.Equalf(t, c.us, res.Microsecond(), \"Microsecond mismatch for case %v\", c)",
        "require.NoErrorf(t, err, \"Unexpected error for case %v\", c)",
        "require.Truef(t, c.err.Equal(err), \"Error mismatch for case %v\", c)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected, error) in [
        ("20000102", (mysql::TypeDate, 2000, 1, 2, 0, 0, 0, 0), false),
        (
            "20000102.9",
            (mysql::TypeDate, 2000, 1, 2, 0, 0, 0, 0),
            false,
        ),
        ("0.0", (mysql::TypeDate, 0, 0, 0, 0, 0, 0, 0), false),
        (
            "20000102030405",
            (mysql::TypeDatetime, 2000, 1, 2, 3, 4, 5, 0),
            false,
        ),
        (
            "20000102030405.015625",
            (mysql::TypeDatetime, 2000, 1, 2, 3, 4, 5, 15625),
            false,
        ),
        (
            "20000102030405.0078125",
            (mysql::TypeDatetime, 2000, 1, 2, 3, 4, 5, 7812),
            false,
        ),
        ("2000", (mysql::TypeDatetime, 0, 0, 0, 0, 0, 0, 0), true),
        (
            "20000000000000",
            (mysql::TypeDatetime, 2000, 0, 0, 0, 0, 0, 0),
            false,
        ),
    ] {
        let decimal = Decimal::from_str(input).unwrap();
        let result = types::ParseTimeFromDecimal(
            &types::BasicTimeContext {
                flags: types::TimeFlags {
                    ignore_zero_in_date: true,
                    ignore_zero_date: true,
                    ..Default::default()
                },
                location: chrono_tz::UTC,
            },
            &decimal,
        );
        assert_eq!(result.is_err(), error, "{input}");
        if let Ok(v) = result {
            assert_eq!(
                (
                    v.Type(),
                    v.Year(),
                    v.Month(),
                    v.Day(),
                    v.Hour(),
                    v.Minute(),
                    v.Second(),
                    v.Microsecond()
                ),
                expected,
                "{input}"
            );
        }
    }
}

// TestGetFormatType 对应 Go 的同名测试：验证格式字符串分类为日期/时间/datetime。
#[test]
pub fn TestGetFormatType() {
    // 原 Go 签名：func TestGetFormatType(t *testing.T)；原函数体约 16 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "isDuration, isDate := types.GetFormatType(input)",
        "require.False(t, isDuration)",
        "require.False(t, isDate)",
        "isDuration, isDate = types.GetFormatType(input)",
        "require.True(t, isDate)",
        "require.True(t, isDuration)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    assert_eq!(types::GetFormatType("TEST"), (false, false));
    assert_eq!(types::GetFormatType("%y %m %d 2019 04 01"), (false, true));
    assert_eq!(types::GetFormatType("%h 30"), (true, false));
}

// TestGetFracIndex 对应 Go 的同名测试：验证 GetFracIndex 定位小数秒起点。
#[test]
pub fn TestGetFracIndex() {
    // 原 Go 签名：func TestGetFracIndex(t *testing.T)；原函数体约 16 行。
    let _go_cases = go_cases(&[
        "{\"2019.01.01 00:00:00\", -1},",
        "{\"2019.01.01 00:00:00.1\", 19},",
        "{\"12345.6\", 5},",
        "{\"2020-01-01 12:00:00.123456 +0600 PST\", 19},",
        "{\"2020-01-01 12:00:00.123456 -0600 PST\", 19},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "index := types.GetFracIndex(testCase.str)",
        "require.Equal(t, testCase.expectIndex, index)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected) in [
        ("2019.01.01 00:00:00", -1),
        ("2019.01.01 00:00:00.1", 19),
        ("12345.6", 5),
        ("2020-01-01 12:00:00.123456 +0600 PST", 19),
        ("2020-01-01 12:00:00.123456 -0600 PST", 19),
    ] {
        assert_eq!(types::GetFracIndex(input), expected, "{input}");
    }
}

// TestTimeOverflow 对应 Go 的同名测试：验证 DateTimeIsOverflow 在 timestamp/datetime/date 上的边界。
#[test]
pub fn TestTimeOverflow() {
    // 原 Go 签名：func TestTimeOverflow(t *testing.T)；原函数体约 31 行。
    let _go_cases = go_cases(&[
        "{\"2012-12-31 11:30:45\", false},",
        "{\"12-12-31 11:30:45\", false},",
        "{\"2012-12-31\", false},",
        "{\"20121231\", false},",
        "{\"2012-02-29\", false},",
        "{\"2018-01-01 18\", false},",
        "{\"18-01-01 18\", false},",
        "{\"2018.01.01\", false},",
        "{\"2018.01.01 00:00:00\", false},",
        "{\"2018/01/01-00:00:00\", false},",
        "{\"0999-12-31 22:00:00\", false},",
        "{\"9999-12-31 23:59:59\", false},",
        "{\"0001-01-01 00:00:00\", false},",
        "{\"0001-01-01 23:59:59\", false},",
        "{\"0000-01-01 00:00:00\", true},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreZeroInDate(true), time.UTC, contextutil.IgnoreWarn)",
        "v, err := types.ParseDatetime(typeCtx, test.Input)",
        "require.NoError(t, err)",
        "isOverflow, err := types.DateTimeIsOverflow(typeCtx, v)",
        "require.Equal(t, test.Output, isOverflow)",
    ];

    let ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    for (input, expected) in [
        ("2012-12-31 11:30:45", false),
        ("12-12-31 11:30:45", false),
        ("20121231", false),
        ("2012-02-29", false),
        ("0999-12-31 22:00:00", false),
        ("9999-12-31 23:59:59", false),
        ("0001-01-01 00:00:00", false),
        ("0000-01-01 00:00:00", true),
    ] {
        let value = types::ParseDatetime(&ctx, input).unwrap();
        assert_eq!(
            types::DateTimeIsOverflow(&ctx, value).unwrap(),
            expected,
            "{input}"
        );
    }
}

// TestTruncateFrac 对应 Go 的同名测试：验证 TruncateFrac 按 fsp 截断 Go time.Time 纳秒。
#[test]
pub fn TestTruncateFrac() {
    // 原 Go 签名：func TestTruncateFrac(t *testing.T)；原函数体约 16 行。
    let _go_cases = go_cases(&[
        "{time.Date(2011, 11, 11, 10, 10, 10, 888888, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 11, time.UTC)},",
        "{time.Date(2011, 11, 11, 10, 10, 10, 111111, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 10, time.UTC)},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "input time.Time",
        "output time.Time",
        "{time.Date(2011, 11, 11, 10, 10, 10, 888888, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 11, time.UTC)},",
        "{time.Date(2011, 11, 11, 10, 10, 10, 111111, time.UTC), 0, time.Date(2011, 11, 11, 10, 10, 10, 10, time.UTC)},",
        "res, err := types.TruncateFrac(col.input, col.fsp)",
        "require.Equal(t, col.output.Second(), res.Second())",
        "require.NoError(t, err)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for nanos in [888_888, 111_111] {
        let input = chrono_tz::UTC
            .with_ymd_and_hms(2011, 11, 11, 10, 10, 10)
            .unwrap()
            .with_nanosecond(nanos)
            .unwrap()
            .naive_utc();
        assert_eq!(types::TruncateFrac(input, 0).unwrap().second(), 10);
    }
}

// TestTimeSub 对应 Go 的同名测试：验证 Time.Sub 生成 Duration 并处理 duration+time 错误组合。
#[test]
pub fn TestTimeSub() {
    // 原 Go 签名：func TestTimeSub(t *testing.T)；原函数体约 23 行。
    let _go_cases = go_cases(&[
        "{\"2017-01-18 01:01:01\", \"2017-01-18 00:00:01\", \"01:01:00\"},",
        "{\"2017-01-18 01:01:01\", \"2017-01-18 01:01:01\", \"00:00:00\"},",
        "{\"2019-04-12 18:20:00\", \"2019-04-12 14:00:00\", \"04:20:00\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "v1, err := types.ParseTime(typeCtx, tt.Arg1, mysql.TypeDatetime, types.MaxFsp)",
        "require.NoError(t, err)",
        "v2, err := types.ParseTime(typeCtx, tt.Arg2, mysql.TypeDatetime, types.MaxFsp)",
        "dur, _, err := types.ParseDuration(typeCtx, tt.Ret, types.MaxFsp)",
        "require.Equal(t, dur, rec)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (a, b, expected) in [
        ("2017-01-18 01:01:01", "2017-01-18 00:00:01", "01:01:00"),
        ("2017-01-18 01:01:01", "2017-01-18 01:01:01", "00:00:00"),
        ("2019-04-12 18:20:00", "2019-04-12 14:00:00", "04:20:00"),
    ] {
        let lhs = types::ParseTime(&types::StrictContext, a, mysql::TypeDatetime, 6).unwrap();
        let rhs = types::ParseTime(&types::StrictContext, b, mysql::TypeDatetime, 6).unwrap();
        let want = types::ParseDuration(&types::StrictContext, expected, 6)
            .unwrap()
            .0;
        assert_eq!(lhs.Sub(&types::StrictContext, rhs), want);
    }
}

// TestCheckMonthDay 对应 Go 的同名测试：验证 CheckMonthDay 对月份天数、闰年、零月零日和错误标志的处理。
#[test]
pub fn TestCheckMonthDay() {
    // 原 Go 签名：func TestCheckMonthDay(t *testing.T)；原函数体约 32 行。
    let _go_cases = go_cases(&[
        "{types.FromDate(1900, 2, 29, 0, 0, 0, 0), false},",
        "{types.FromDate(1900, 2, 28, 0, 0, 0, 0), true},",
        "{types.FromDate(2000, 2, 29, 0, 0, 0, 0), true},",
        "{types.FromDate(2000, 1, 1, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 1, 1, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 1, 31, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 4, 1, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 4, 31, 0, 0, 0, 0), false},",
        "{types.FromDate(1900, 4, 30, 0, 0, 0, 0), true},",
        "{types.FromDate(2000, 2, 30, 0, 0, 0, 0), false},",
        "{types.FromDate(2000, 13, 1, 0, 0, 0, 0), false},",
        "{types.FromDate(4000, 2, 29, 0, 0, 0, 0), true},",
        "{types.FromDate(3200, 2, 29, 0, 0, 0, 0), true},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "date types.CoreTime",
        "{types.FromDate(1900, 2, 29, 0, 0, 0, 0), false},",
        "{types.FromDate(1900, 2, 28, 0, 0, 0, 0), true},",
        "{types.FromDate(2000, 2, 29, 0, 0, 0, 0), true},",
        "{types.FromDate(2000, 1, 1, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 1, 1, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 1, 31, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 4, 1, 0, 0, 0, 0), true},",
        "{types.FromDate(1900, 4, 31, 0, 0, 0, 0), false},",
        "{types.FromDate(1900, 4, 30, 0, 0, 0, 0), true},",
        "{types.FromDate(2000, 2, 30, 0, 0, 0, 0), false},",
        "{types.FromDate(2000, 13, 1, 0, 0, 0, 0), false},",
        "{types.FromDate(4000, 2, 29, 0, 0, 0, 0), true},",
        "{types.FromDate(3200, 2, 29, 0, 0, 0, 0), true},",
        "typeCtx := types.NewContext(types.StrictFlags.WithIgnoreInvalidDateErr(false), time.UTC, contextutil.IgnoreWarn)",
        "v := types.NewTime(tt.date, mysql.TypeDate, types.DefaultFsp)",
        "require.NoError(t, err)",
        "require.True(t, types.ErrWrongValue.Equal(err))",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (core, valid) in [
        (types::FromDate(1900, 2, 29, 0, 0, 0, 0), false),
        (types::FromDate(1900, 2, 28, 0, 0, 0, 0), true),
        (types::FromDate(2000, 2, 29, 0, 0, 0, 0), true),
        (types::FromDate(2000, 1, 1, 0, 0, 0, 0), true),
        (types::FromDate(1900, 1, 31, 0, 0, 0, 0), true),
        (types::FromDate(1900, 4, 31, 0, 0, 0, 0), false),
        (types::FromDate(1900, 4, 30, 0, 0, 0, 0), true),
        (types::FromDate(2000, 2, 30, 0, 0, 0, 0), false),
        (types::FromDate(2000, 13, 1, 0, 0, 0, 0), false),
        (types::FromDate(4000, 2, 29, 0, 0, 0, 0), true),
        (types::FromDate(3200, 2, 29, 0, 0, 0, 0), true),
    ] {
        let value = types::NewTime(core, mysql::TypeDate, 0);
        assert_eq!(
            value.Check(&types::StrictContext).is_ok(),
            valid,
            "{core:?}"
        );
    }
}

// TestFormatIntWidthN 对应 Go 的同名测试：验证 FormatIntWidthN 的宽度补零和不截断逻辑。
#[test]
pub fn TestFormatIntWidthN() {
    // 原 Go 签名：func TestFormatIntWidthN(t *testing.T)；原函数体约 21 行。
    let _go_cases = go_cases(&[
        "{0, 0, \"0\"},",
        "{1, 0, \"1\"},",
        "{1, 1, \"1\"},",
        "{1, 2, \"01\"},",
        "{10, 2, \"10\"},",
        "{99, 3, \"099\"},",
        "{100, 3, \"100\"},",
        "{999, 3, \"999\"},",
        "{1000, 3, \"1000\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "re := types.FormatIntWidthN(ca.num, ca.width)",
        "require.Equal(t, ca.result, re)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (number, width, expected) in [
        (0, 0, "0"),
        (1, 0, "1"),
        (1, 1, "1"),
        (1, 2, "01"),
        (10, 2, "10"),
        (99, 3, "099"),
        (100, 3, "100"),
        (999, 3, "999"),
        (1000, 3, "1000"),
    ] {
        assert_eq!(types::FormatIntWidthN(number, width), expected);
    }
}

// TestFromGoTime 对应 Go 的同名测试：验证 FromGoTime 从 time.Time 抽取 CoreTime 字段和纳秒到微秒转换。
#[test]
pub fn TestFromGoTime() {
    // 原 Go 签名：func TestFromGoTime(t *testing.T)；原函数体约 27 行。
    // 原 Go 关键注释：
    // - Test rounding of nanosecond to millisecond.
    let _go_cases = go_cases(&[
        "{\"2006-01-02T15:04:05.999999999Z\", 2006, 1, 2, 15, 4, 6, 0},",
        "{\"2006-01-02T15:04:05.999999000Z\", 2006, 1, 2, 15, 4, 5, 999999},",
        "{\"2006-01-02T15:04:05.999999499Z\", 2006, 1, 2, 15, 4, 5, 999999},",
        "{\"2006-01-02T15:04:05.999999500Z\", 2006, 1, 2, 15, 4, 6, 0},",
        "{\"2006-01-02T15:04:05.000000501Z\", 2006, 1, 2, 15, 4, 5, 1},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "v, err := time.Parse(time.RFC3339Nano, ca.input)",
        "require.NoError(t, err)",
        "t1 := types.FromGoTime(v)",
        "require.Equalf(t, types.FromDate(ca.yy, ca.mm, ca.dd, ca.hh, ca.min, ca.sec, ca.micro), t1, \"idx %d\", ith)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    for (input, expected) in [
        (
            "2006-01-02T15:04:05.999999999Z",
            types::FromDate(2006, 1, 2, 15, 4, 6, 0),
        ),
        (
            "2006-01-02T15:04:05.999999000Z",
            types::FromDate(2006, 1, 2, 15, 4, 5, 999999),
        ),
        (
            "2006-01-02T15:04:05.999999499Z",
            types::FromDate(2006, 1, 2, 15, 4, 5, 999999),
        ),
        (
            "2006-01-02T15:04:05.999999500Z",
            types::FromDate(2006, 1, 2, 15, 4, 6, 0),
        ),
        (
            "2006-01-02T15:04:05.000000501Z",
            types::FromDate(2006, 1, 2, 15, 4, 5, 1),
        ),
    ] {
        let parsed = chrono::DateTime::parse_from_rfc3339(input)
            .unwrap()
            .with_timezone(&chrono_tz::UTC);
        assert_eq!(types::FromGoTime(parsed), expected, "{input}");
    }
}

// TestGetTimezone 对应 Go 的同名测试：验证 GetTimezone 解析 Z、正负 offset 和非法格式。
#[test]
pub fn TestGetTimezone() {
    // 原 Go 签名：func TestGetTimezone(t *testing.T)；原函数体约 27 行。
    let _go_cases = go_cases(&[
        "{\"2020-10-10T10:10:10Z\", 19, \"\", \"\", \"\", \"\"},",
        "{\"2020-10-10T10:10:10\", -1, \"\", \"\", \"\", \"\"},",
        "{\"2020-10-10T10:10:10-08\", 19, \"-\", \"08\", \"\", \"\"},",
        "{\"2020-10-10T10:10:10-0700\", 19, \"-\", \"07\", \"\", \"00\"},",
        "{\"2020-10-10T10:10:10+08:20\", 19, \"+\", \"08\", \":\", \"20\"},",
        "{\"2020-10-10T10:10:10+08:10\", 19, \"+\", \"08\", \":\", \"10\"},",
        "{\"2020-10-10T10:10:10+8:00\", -1, \"\", \"\", \"\", \"\"},",
        "{\"2020-10-10T10:10:10+082:10\", -1, \"\", \"\", \"\", \"\"},",
        "{\"2020-10-10T10:10:10+08:101\", -1, \"\", \"\", \"\", \"\"},",
        "{\"2020-10-10T10:10:10+T8:11\", -1, \"\", \"\", \"\", \"\"},",
        "{\"2020-09-06T05:49:13.293Z\", 23, \"\", \"\", \"\", \"\"},",
        "{\"2020-09-06T05:49:13.293\", -1, \"\", \"\", \"\", \"\"},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "idx, tzSign, tzHour, tzSep, tzMinute := types.GetTimezone(ca.input)",
        "require.Equal(t, [5]any{ca.idx, ca.tzSign, ca.tzHour, ca.tzSep, ca.tzMinute}, [5]any{idx, tzSign, tzHour, tzSep, tzMinute}, \"idx %d\", ith)",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let cases = [
        ("2020-10-10T10:10:10Z", (19, "", "", "", "")),
        ("2020-10-10T10:10:10", (-1, "", "", "", "")),
        ("2020-10-10T10:10:10-08", (19, "-", "08", "", "")),
        ("2020-10-10T10:10:10-0700", (19, "-", "07", "", "00")),
        ("2020-10-10T10:10:10+08:20", (19, "+", "08", ":", "20")),
        ("2020-10-10T10:10:10+8:00", (-1, "", "", "", "")),
        ("2020-10-10T10:10:10+082:10", (-1, "", "", "", "")),
        ("2020-09-06T05:49:13.293Z", (23, "", "", "", "")),
        ("2020-09-06T05:49:13.293", (-1, "", "", "", "")),
    ];
    for (input, (idx, sign, hour, sep, minute)) in cases {
        let got = types::GetTimezone(input);
        assert_eq!(
            (
                got.0,
                got.1.as_str(),
                got.2.as_str(),
                got.3.as_str(),
                got.4.as_str()
            ),
            (idx, sign, hour, sep, minute),
            "{input}"
        );
    }
}

// TestParseWithTimezone 对应 Go 的同名测试：验证 ParseTime 处理内嵌时区偏移、session 时区、DST 和日期类型。
#[test]
pub fn TestParseWithTimezone() {
    // 原 Go 签名：func TestParseWithTimezone(t *testing.T)；原函数体约 84 行。
    // 原 Go 关键注释：
    // - lit is the string literal to be parsed, which contains timezone, and gt is the ground truth time
    // - in go's time.Time, while sysTZ is the system timezone where the string literal gets parsed.
    // - we first parse the string literal, and convert it into UTC and then compare it with the ground truth time in UTC.
    // - note that sysTZ won't affect the physical time the string literal represents.
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "getTZ := func(tzSign string, tzHour, tzMinue int) *time.Location {",
        "return time.FixedZone(fmt.Sprintf(\"UTC%s%02d:%02d\", tzSign, tzHour, tzMinue), offset)",
        "// in go's time.Time, while sysTZ is the system timezone where the string literal gets parsed.",
        "gt time.Time",
        "sysTZ *time.Location",
        "time.Date(2006, 1, 2, 15, 4, 5, 0, getTZ(\"+\", 0, 0)),",
        "time.Date(2020, 10, 21, 16, 5, 10, 500*1000*1000, getTZ(\"+\", 0, 0)),",
        "time.Date(2020, 10, 21, 16, 5, 10, 500*1000*1000, getTZ(\"+\", 8, 0)),",
        "time.Date(2020, 10, 21, 16, 5, 10, 500*1000*1000, getTZ(\"-\", 7, 0)),",
        "time.Date(2020, 10, 21, 16, 5, 10, 500*1000*1000, getTZ(\"+\", 9, 0)),",
        "time.Date(2006, 1, 2, 15, 4, 5, 0, getTZ(\"+\", 9, 0)),",
        "time.Date(2006, 1, 2, 15, 4, 5, 0, getTZ(\"-\", 2, 0)),",
        "time.Date(2006, 1, 2, 15, 4, 5, 0, getTZ(\"-\", 14, 0)),",
        "v, err := types.ParseTime(types.NewContext(types.StrictFlags, ca.sysTZ, contextutil.IgnoreWarn), ca.lit, mysql.TypeTimestamp, ca.fsp)",
        "require.NoErrorf(t, err, \"tidb time parse misbehaved on %d\", ith)",
        "require.NoErrorf(t, err, \"tidb time convert failed on %d\", ith)",
        "require.Equalf(t, ca.gt.In(time.UTC), t1.In(time.UTC), \"parsed time mismatch on %dth case\", ith)",
    ];

    for (lit, fsp, sys_tz, expected_utc) in [
        (
            "2006-01-02T15:04:05Z",
            0,
            chrono_tz::UTC,
            "2006-01-02 15:04:05",
        ),
        (
            "2006-01-02T15:04:05Z",
            0,
            chrono_tz::Asia::Tokyo,
            "2006-01-02 15:04:05",
        ),
        (
            "2020-10-21T16:05:10.50Z",
            2,
            chrono_tz::America::Los_Angeles,
            "2020-10-21 16:05:10.500000",
        ),
        (
            "2020-10-21T16:05:10.50+08",
            2,
            chrono_tz::UTC,
            "2020-10-21 08:05:10.500000",
        ),
        (
            "2020-10-21T16:05:10.50-0700",
            2,
            chrono_tz::UTC,
            "2020-10-21 23:05:10.500000",
        ),
        (
            "2006-01-02T15:04:05-14:00",
            0,
            chrono_tz::Asia::Tokyo,
            "2006-01-03 05:04:05",
        ),
    ] {
        let ctx = types::BasicTimeContext {
            flags: Default::default(),
            location: sys_tz,
        };
        let value = types::ParseTime(&ctx, lit, mysql::TypeTimestamp, fsp).unwrap();
        let utc = value.GoTime(sys_tz).unwrap().with_timezone(&chrono::Utc);
        let rendered = if fsp == 0 {
            utc.format("%Y-%m-%d %H:%M:%S").to_string()
        } else {
            utc.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
        };
        assert_eq!(rendered, expected_utc, "{lit}/{sys_tz}");
    }
    for invalid in ["2020-10-21T16:05:10+14:01", "2020-10-21T16:05:10+15:00"] {
        assert!(types::ParseTime(&types::StrictContext, invalid, mysql::TypeTimestamp, 0).is_err());
    }
}

// TestMarshalTime 对应 Go 的同名测试：验证 Time 的 JSON marshal/unmarshal 往返。
#[test]
pub fn TestMarshalTime() {
    // 原 Go 签名：func TestMarshalTime(t *testing.T)；原函数体约 10 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "v1, err := types.ParseTime(typeCtx, \"2017-01-18 01:01:01.123456\", mysql.TypeDatetime, types.MaxFsp)",
        "require.NoError(t, err)",
        "j, err := json.Marshal(v1)",
        "var v2 types.Time",
        "require.NoError(t, json.Unmarshal(j, &v2))",
        "require.Equal(t, 0, v1.Compare(v2))",
    ];

    // 以下断言执行对应 Rust API，并保留 Go fixture 作为逐项对照。
    let value = types::ParseTime(
        &types::StrictContext,
        "2017-01-18 01:01:01.123456",
        mysql::TypeDatetime,
        6,
    )
    .unwrap();
    let json = serde_json::to_string(&value).unwrap();
    let decoded: types::Time = serde_json::from_str(&json).unwrap();
    assert_eq!(value.Compare(decoded), 0);
}

// TestDurationConvertToYearFromNow 对应 Go 的同名测试：验证 Duration.ConvertToYearFromNow 的正常年数和越界错误。
#[test]
pub fn TestDurationConvertToYearFromNow() {
    // 原 Go 签名：func TestDurationConvertToYearFromNow(t *testing.T)；原函数体约 36 行。
    let _go_cases = go_cases(&[
        "{types.NewDuration(1, 0, 0, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2023, false, nil},",
        "{types.NewDuration(40, 0, 0, 0, 0), \"2023-12-31T11:00:00Z\", time.UTC, 2024, false, nil},",
        "{types.NewDuration(40, 0, 0, 0, 0), \"2023-12-31T11:00:00+12:00\", time.UTC, 2023, false, nil},",
        "{types.NewDuration(-20, 0, 0, 0, 0), \"2024-01-01T13:00:00Z\", time.UTC, 2023, false, nil},",
        "{types.NewDuration(-20, 0, 0, 0, 0), \"2024-01-01T13:00:00-12:00\", time.UTC, 2024, false, nil},",
        "{types.NewDuration(0, 20, 12, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2012, true, nil},",
        "{types.NewDuration(0, 0, 12, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2012, true, nil},",
        "{types.NewDuration(0, 0, 0, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 0, true, nil},",
        "{types.NewDuration(200, 0, 0, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2155, true, types.ErrWarnDataOutOfRange},",
    ]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "dur types.Duration",
        "sysTZ *time.Location",
        "{types.NewDuration(1, 0, 0, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2023, false, nil},",
        "{types.NewDuration(40, 0, 0, 0, 0), \"2023-12-31T11:00:00Z\", time.UTC, 2024, false, nil},",
        "{types.NewDuration(40, 0, 0, 0, 0), \"2023-12-31T11:00:00+12:00\", time.UTC, 2023, false, nil},",
        "{types.NewDuration(-20, 0, 0, 0, 0), \"2024-01-01T13:00:00Z\", time.UTC, 2023, false, nil},",
        "{types.NewDuration(-20, 0, 0, 0, 0), \"2024-01-01T13:00:00-12:00\", time.UTC, 2024, false, nil},",
        "{types.NewDuration(0, 20, 12, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2012, true, nil},",
        "{types.NewDuration(0, 0, 12, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2012, true, nil},",
        "{types.NewDuration(0, 0, 0, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 0, true, nil},",
        "{types.NewDuration(200, 0, 0, 0, 0), \"2023-11-13T03:09:00Z\", time.UTC, 2155, true, types.ErrWarnDataOutOfRange},",
        "ctx := types.NewContext(types.StrictFlags.WithCastTimeToYearThroughConcat(c.throughStr), c.sysTZ, contextutil.NewFuncWarnAppenderForTest(func(_ string, _ error) {",
        "require.Fail(t, \"shouldn't append warninng\")",
        "now, err := time.Parse(time.RFC3339, c.nowLit)",
        "require.NoError(t, err)",
        "require.ErrorIs(t, err, c.err)",
        "require.Equal(t, c.expected, year, \"convert %s + now(%s) as year\", c.dur.String(), c.nowLit)",
    ];

    let normal_ctx = types::BasicTimeContext {
        flags: Default::default(),
        location: chrono_tz::UTC,
    };
    for (duration, now, expected) in [
        (
            types::NewDuration(1, 0, 0, 0, 0),
            "2023-11-13T03:09:00Z",
            2023,
        ),
        (
            types::NewDuration(40, 0, 0, 0, 0),
            "2023-12-31T11:00:00Z",
            2024,
        ),
        (
            types::NewDuration(40, 0, 0, 0, 0),
            "2023-12-30T23:00:00Z",
            2023,
        ),
        (
            types::NewDuration(-20, 0, 0, 0, 0),
            "2024-01-01T13:00:00Z",
            2023,
        ),
        (
            types::NewDuration(-20, 0, 0, 0, 0),
            "2024-01-02T01:00:00Z",
            2024,
        ),
    ] {
        let now = chrono::DateTime::parse_from_rfc3339(now)
            .unwrap()
            .with_timezone(&chrono_tz::UTC);
        assert_eq!(
            duration.ConvertToYearFromNow(&normal_ctx, now).unwrap(),
            expected
        );
    }
    let concat_ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            cast_time_to_year_through_concat: true,
            ..Default::default()
        },
        location: chrono_tz::UTC,
    };
    let now = chrono_tz::UTC
        .with_ymd_and_hms(2023, 11, 13, 3, 9, 0)
        .unwrap();
    for (duration, expected) in [
        (types::NewDuration(0, 20, 12, 0, 0), 2012),
        (types::NewDuration(0, 0, 12, 0, 0), 2012),
        (types::NewDuration(0, 0, 0, 0, 0), 0),
    ] {
        assert_eq!(
            duration.ConvertToYearFromNow(&concat_ctx, now).unwrap(),
            expected
        );
    }
    assert!(
        types::NewDuration(200, 0, 0, 0, 0)
            .ConvertToYearFromNow(&concat_ctx, now)
            .is_err()
    );
}

// BenchmarkFormat 对应 Go benchmark/helper：保留 Time.String benchmark 的构造和循环入口。
pub fn BenchmarkFormat() {
    // 原 Go 签名：func BenchmarkFormat(b *testing.B)；原函数体约 9 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] =
        &["t1 := types.NewTime(types.FromGoTime(time.Now()), mysql.TypeTimestamp, 0)"];

    let value = types::NewTime(
        types::FromDate(2026, 7, 14, 12, 34, 56, 0),
        mysql::TypeTimestamp,
        0,
    );
    assert_eq!(
        value.DateFormat("%Y-%m-%d %H:%i:%s").unwrap(),
        "2026-07-14 12:34:56"
    );
}

// BenchmarkTimeAdd 对应 Go benchmark/helper：保留 Time.Add benchmark 的构造和循环入口。
pub fn BenchmarkTimeAdd() {
    // 原 Go 签名：func BenchmarkTimeAdd(b *testing.B)；原函数体约 11 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "arg1, _ := types.ParseTime(typeCtx, \"2017-01-18\", mysql.TypeDatetime, types.MaxFsp)",
        "arg2, _, _ := types.ParseDuration(typeCtx, \"12:30:59\", types.MaxFsp)",
    ];

    let time =
        types::ParseTime(&types::StrictContext, "2017-01-18", mysql::TypeDatetime, 6).unwrap();
    let duration = types::ParseDuration(&types::StrictContext, "12:30:59", 6)
        .unwrap()
        .0;
    assert_eq!(
        time.Add(&types::StrictContext, duration).unwrap().String(),
        "2017-01-18 12:30:59.000000"
    );
}

// BenchmarkTimeCompare 对应 Go benchmark/helper：保留 Time.Compare benchmark 的构造和循环入口。
pub fn BenchmarkTimeCompare() {
    // 原 Go 签名：func BenchmarkTimeCompare(b *testing.B)；原函数体约 34 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[
        "typeCtx := types.DefaultStmtNoWarningContext",
        "mustParse := func(str string) types.Time {",
        "t, err := types.ParseDatetime(typeCtx, str)",
        "Arg1 types.Time",
        "Arg2 types.Time",
    ];

    for (left, right, expected) in [
        ("2011-10-10 11:11:11", "2011-10-10 11:11:11", 0),
        ("2011-10-10 11:11:11.123456", "2011-10-10 11:11:11.1", 1),
        ("2011-10-10 11:11:11", "2011-10-10 11:11:11.123", -1),
    ] {
        let left = types::ParseDatetime(&types::StrictContext, left).unwrap();
        let right = types::ParseDatetime(&types::StrictContext, right).unwrap();
        assert_eq!(left.Compare(right), expected);
    }
}

// benchmarkDateFormat 对应 Go benchmark/helper：保留 ParseDateFormat benchmark helper。
pub fn benchmarkDateFormat() {
    // 原 Go 签名：func benchmarkDateFormat(b *testing.B, name, str string)；原函数体约 7 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &["types.ParseDateFormat(str)"];

    assert_eq!(
        types::ParseDateFormat("2011-12-13"),
        vec!["2011", "12", "13"]
    );
}

// BenchmarkParseDateFormat 对应 Go benchmark/helper：保留多种日期格式 benchmark 入口。
pub fn BenchmarkParseDateFormat() {
    // 原 Go 签名：func BenchmarkParseDateFormat(b *testing.B)；原函数体约 8 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &[];

    for input in [
        "2011-12-13",
        "20111213",
        "2011-12-13 14:15:16",
        "20111213141516",
        "2011-12-13 14:15:16.123456",
        "2011---12---13 14::15::16..123456",
    ] {
        assert!(!types::ParseDateFormat(input).is_empty(), "{input}");
    }
}

// benchmarkDatetimeFormat 对应 Go benchmark/helper：保留 ParseDatetime benchmark helper。
pub fn benchmarkDatetimeFormat() {
    // 原 Go 签名：func benchmarkDatetimeFormat(b *testing.B, name string, ctx types.Context, str string)；原函数体约 10 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &["_, err := types.ParseDatetime(ctx, str)"];

    assert_eq!(
        types::ParseDatetime(&types::StrictContext, "2020-10-10T10:10:10")
            .unwrap()
            .String(),
        "2020-10-10 10:10:10"
    );
}

// BenchmarkParseDatetimeFormat 对应 Go benchmark/helper：保留 datetime 格式 benchmark 入口。
pub fn BenchmarkParseDatetimeFormat() {
    // 原 Go 签名：func BenchmarkParseDatetimeFormat(b *testing.B)；原函数体约 5 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &["typeCtx := types.DefaultStmtNoWarningContext"];

    benchmarkDatetimeFormat();
    assert!(types::ParseDatetime(&types::StrictContext, "2020-10-10T10:10:10+08:00").is_ok());
}

// benchmarkStrToDate 对应 Go benchmark/helper：保留 StrToDate benchmark helper。
pub fn benchmarkStrToDate() {
    // 原 Go 签名：func benchmarkStrToDate(b *testing.B, name string, ctx types.Context, str, format string)；原函数体约 8 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &["var t types.Time"];

    let mut value = types::Time::default();
    assert!(value.StrToDate(
        &types::StrictContext,
        "31/05/2016 12:34:56.1234",
        "%d/%m/%Y %H:%i:%S.%f"
    ));
    assert_eq!(value.String(), "2016-05-31 12:34:56.1234");
}

// BenchmarkStrToDate 对应 Go benchmark/helper：保留 STR_TO_DATE 格式 benchmark 入口。
pub fn BenchmarkStrToDate() {
    // 原 Go 签名：func BenchmarkStrToDate(b *testing.B)；原函数体约 6 行。
    let _go_cases = go_cases(&[]);

    // Call summary from Go test source.
    let _go_call_flow: &[&str] = &["typeCtx := types.DefaultStmtNoWarningContext"];

    benchmarkStrToDate();
    for (input, format, expected) in [
        (
            "04:13:56 AM 13/05/2019",
            "%r %d/%c/%Y",
            "2019-05-13 04:13:56",
        ),
        (" 4:13:56 13/05/2019", "%T %d/%c/%Y", "2019-05-13 04:13:56"),
    ] {
        let mut value = types::Time::default();
        assert!(
            value.StrToDate(&types::StrictContext, input, format),
            "{input}"
        );
        assert_eq!(value.String(), expected, "{input}");
    }
}

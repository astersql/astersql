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

// 时间格式化与字符串解析日期的单元测试。
//
// 对照 Go `pkg/types/format_test.go`，覆盖 `Time::DateFormat` 与
// `Time::StrToDate` 的格式符、零日期与非法日期边界。

// 对照 pkg/types/format_test.go，覆盖 Time.DateFormat 与 Time.StrToDate。
//

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use crate as integration_types;

use integration_types::metadata::mysql;
use integration_types::time::CoreTime;

/// 测试用时间类型再导出，并提供零值 CoreTime。
mod types {
    pub use super::integration_types::time::*;

    /// 全零压缩时间戳（年=月=日=0）。
    pub const ZeroCoreTime: CoreTime = CoreTime(0);
}

/// DateFormat 表驱动用例：输入时间串、格式串与期望输出。
struct TimeFormatCase {
    input: &'static str,
    format: &'static str,
    expect: &'static str,
}

/// `DateFormat` 全格式符与零/非法日期边界。
// TestTimeFormatMethod 对应 Go 的 DateFormat 表驱动测试。
// 前两组覆盖完整格式符，后续保留零日期、非法日期和字面量混排等 MySQL 兼容边界。
#[test]
fn TestTimeFormatMethod() {
    let mut typeCtx = types::BasicTimeContext::default();
    typeCtx.flags.ignore_zero_in_date = true;
    let tblDate = vec![
        TimeFormatCase {
            input: "2010-01-07 23:12:34.12345",
            format: "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y %%",
            expect: "Jan January 01 1 7th 07 7 007 23 11 12 PM 11:12:34 PM 23:12:34 34 123450 01 01 01 01 Thu Thursday 4 2010 2010 2010 10 %",
        },
        TimeFormatCase {
            input: "2012-12-21 23:12:34.123456",
            format: "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y %%",
            expect: "Dec December 12 12 21st 21 21 356 23 11 12 PM 11:12:34 PM 23:12:34 34 123456 51 51 51 51 Fri Friday 5 2012 2012 2012 12 %",
        },
        TimeFormatCase {
            input: "0000-01-01 00:00:00.123456",
            // Go 注释：week()/yearweek() 不支持 multi mode，所以部分结果和 MySQL 不同。
            format: "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %v %Y %y %%",
            expect: "Jan January 01 1 1st 01 1 001 0 12 00 AM 12:00:00 AM 00:00:00 00 123456 52 0000 00 %",
        },
        TimeFormatCase {
            input: "2016-09-3 00:59:59.123456",
            format: "abc%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y!123 %%xyz %z",
            expect: "abcSep September 09 9 3rd 03 3 247 0 12 59 AM 12:59:59 AM 00:59:59 59 123456 35 35 35 35 Sat Saturday 6 2016 2016 2016 16!123 %xyz z",
        },
        TimeFormatCase {
            input: "2012-10-01 00:00:00",
            format: "%b %M %m %c %D %d %e %j %k %H %i %p %r %T %s %f %v %x %Y %y %%",
            expect: "Oct October 10 10 1st 01 1 275 0 00 00 AM 12:00:00 AM 00:00:00 00 000000 40 2012 2012 12 %",
        },
        TimeFormatCase {
            input: "0000-01-00 00:00:00.123456",
            // Go 注释：非法日期下 MySQL 的 Week()/DateFormat 行为不一致，TiDB 用户不应依赖这些边界。
            format: "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y %%",
            expect: "Jan January 01 1 0th 00 0 000 0 12 00 AM 12:00:00 AM 00:00:00 00 123456 00 00 00 52 Fri Friday 5 4294967295 4294967295 0000 00 %",
        },
    ];

    for (i, tt) in tblDate.iter().enumerate() {
        let tm = types::ParseTime(&typeCtx, tt.input, mysql::TypeDatetime, 6)
            .unwrap_or_else(|error| panic!("Parse time fail: {}: {error}", tt.input));
        let formatted = tm
            .DateFormat(tt.format)
            .unwrap_or_else(|error| panic!("time format fail: {i}: {error}"));
        assert_eq!(tt.expect, formatted, "case {i}");
    }
}

/// StrToDate 成功用例：输入、格式与期望 CoreTime。
struct StrToDateCase {
    input: &'static str,
    format: &'static str,
    expect: CoreTime,
}

/// StrToDate 失败用例：格式错配或非法日期。
struct StrToDateErrCase {
    input: &'static str,
    format: &'static str,
}

/// StrToDate 正反用例：IgnoreInvalidDate 开/关下的解析行为。
// TestStrToDate 对应 Go 的 StrToDate 正反两组表驱动测试。
// 成功表在 IgnoreInvalidDateErr=true 下运行；错误表切回 false，保留格式错配和非法日期分支。
#[test]
fn TestStrToDate() {
    let mut typeCtx = types::BasicTimeContext::default();
    typeCtx.flags.ignore_zero_in_date = true;
    let tests = vec![
        StrToDateCase {
            input: "01,05,2013",
            format: "%d,%m,%Y",
            expect: types::FromDate(2013, 5, 1, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "5 12 2021",
            format: "%m%d%Y",
            expect: types::FromDate(2021, 5, 12, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "May 01, 2013",
            format: "%M %d,%Y",
            expect: types::FromDate(2013, 5, 1, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "a09:30:17",
            format: "a%h:%i:%s",
            expect: types::FromDate(0, 0, 0, 9, 30, 17, 0),
        },
        StrToDateCase {
            input: "09:30:17a",
            format: "%h:%i:%s",
            expect: types::FromDate(0, 0, 0, 9, 30, 17, 0),
        },
        StrToDateCase {
            input: "12:43:24",
            format: "%h:%i:%s",
            expect: types::FromDate(0, 0, 0, 0, 43, 24, 0),
        },
        StrToDateCase {
            input: "abc",
            format: "abc",
            expect: types::ZeroCoreTime,
        },
        StrToDateCase {
            input: "09",
            format: "%m",
            expect: types::FromDate(0, 9, 0, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "09",
            format: "%s",
            expect: types::FromDate(0, 0, 0, 0, 0, 9, 0),
        },
        StrToDateCase {
            input: "12:43:24 AM",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 0, 43, 24, 0),
        },
        StrToDateCase {
            input: "12:43:24 PM",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 12, 43, 24, 0),
        },
        StrToDateCase {
            input: "11:43:24 PM",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 23, 43, 24, 0),
        },
        StrToDateCase {
            input: "00:12:13",
            format: "%T",
            expect: types::FromDate(0, 0, 0, 0, 12, 13, 0),
        },
        StrToDateCase {
            input: "23:59:59",
            format: "%T",
            expect: types::FromDate(0, 0, 0, 23, 59, 59, 0),
        },
        StrToDateCase {
            input: "00/00/0000",
            format: "%m/%d/%Y",
            expect: types::ZeroCoreTime,
        },
        StrToDateCase {
            input: "04/30/2004",
            format: "%m/%d/%Y",
            expect: types::FromDate(2004, 4, 30, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "15:35:00",
            format: "%H:%i:%s",
            expect: types::FromDate(0, 0, 0, 15, 35, 0, 0),
        },
        StrToDateCase {
            input: "Jul 17 33",
            format: "%b %k %S",
            expect: types::FromDate(0, 7, 0, 17, 0, 33, 0),
        },
        StrToDateCase {
            input: "2016-January:7 432101",
            format: "%Y-%M:%l %f",
            expect: types::FromDate(2016, 1, 0, 7, 0, 0, 432101),
        },
        StrToDateCase {
            input: "10:13 PM",
            format: "%l:%i %p",
            expect: types::FromDate(0, 0, 0, 22, 13, 0, 0),
        },
        StrToDateCase {
            input: "12:00:00 AM",
            format: "%h:%i:%s %p",
            expect: types::FromDate(0, 0, 0, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "12:00:00 PM",
            format: "%h:%i:%s %p",
            expect: types::FromDate(0, 0, 0, 12, 0, 0, 0),
        },
        StrToDateCase {
            input: "12:00:00 PM",
            format: "%I:%i:%s %p",
            expect: types::FromDate(0, 0, 0, 12, 0, 0, 0),
        },
        StrToDateCase {
            input: "1:00:00 PM",
            format: "%h:%i:%s %p",
            expect: types::FromDate(0, 0, 0, 13, 0, 0, 0),
        },
        StrToDateCase {
            input: "18/10/22",
            format: "%y/%m/%d",
            expect: types::FromDate(2018, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "8/10/22",
            format: "%y/%m/%d",
            expect: types::FromDate(2008, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "69/10/22",
            format: "%y/%m/%d",
            expect: types::FromDate(2069, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "70/10/22",
            format: "%y/%m/%d",
            expect: types::FromDate(1970, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "18/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(2018, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "2018/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(2018, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "8/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(2008, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "69/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(2069, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "70/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(1970, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "18/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(2018, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "100/10/22",
            format: "%Y/%m/%d",
            expect: types::FromDate(100, 10, 22, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "09/10/1021",
            format: "%d/%m/%y",
            expect: types::FromDate(2010, 10, 9, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "09/10/1021",
            format: "%d/%m/%Y",
            expect: types::FromDate(1021, 10, 9, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: "09/10/10",
            format: "%d/%m/%Y",
            expect: types::FromDate(2010, 10, 9, 0, 0, 0, 0),
        },
        // '%b'/'%M' should be case insensitive
        StrToDateCase {
            input: "31/may/2016 12:34:56.1234",
            format: "%d/%b/%Y %H:%i:%S.%f",
            expect: types::FromDate(2016, 5, 31, 12, 34, 56, 123400),
        },
        StrToDateCase {
            input: "30/april/2016 12:34:56.",
            format: "%d/%M/%Y %H:%i:%s.%f",
            expect: types::FromDate(2016, 4, 30, 12, 34, 56, 0),
        },
        StrToDateCase {
            input: "31/mAy/2016 12:34:56.1234",
            format: "%d/%b/%Y %H:%i:%S.%f",
            expect: types::FromDate(2016, 5, 31, 12, 34, 56, 123400),
        },
        StrToDateCase {
            input: "30/apRil/2016 12:34:56.",
            format: "%d/%M/%Y %H:%i:%s.%f",
            expect: types::FromDate(2016, 4, 30, 12, 34, 56, 0),
        },
        StrToDateCase {
            input: " 04 :13:56 AM13/05/2019",
            format: "%r %d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 4, 13, 56, 0),
        },
        StrToDateCase {
            input: "12: 13:56 AM 13/05/2019",
            format: "%r%d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 0, 13, 56, 0),
        },
        StrToDateCase {
            input: "12:13 :56 pm 13/05/2019",
            format: "%r %d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 12, 13, 56, 0),
        },
        StrToDateCase {
            input: "12:3: 56pm  13/05/2019",
            format: "%r %d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 12, 3, 56, 0),
        },
        StrToDateCase {
            input: "11:13:56",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 11, 13, 56, 0),
        },
        StrToDateCase {
            input: "11:13",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 11, 13, 0, 0),
        },
        StrToDateCase {
            input: "11:",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 11, 0, 0, 0),
        },
        StrToDateCase {
            input: "11",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 11, 0, 0, 0),
        },
        StrToDateCase {
            input: "12",
            format: "%r",
            expect: types::FromDate(0, 0, 0, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: " 4 :13:56 13/05/2019",
            format: "%T %d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 4, 13, 56, 0),
        },
        StrToDateCase {
            input: "23: 13:56  13/05/2019",
            format: "%T%d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 23, 13, 56, 0),
        },
        StrToDateCase {
            input: "12:13 :56 13/05/2019",
            format: "%T %d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 12, 13, 56, 0),
        },
        StrToDateCase {
            input: "19:3: 56  13/05/2019",
            format: "%T %d/%c/%Y",
            expect: types::FromDate(2019, 5, 13, 19, 3, 56, 0),
        },
        StrToDateCase {
            input: "21:13",
            format: "%T",
            expect: types::FromDate(0, 0, 0, 21, 13, 0, 0),
        },
        StrToDateCase {
            input: "21:",
            format: "%T",
            expect: types::FromDate(0, 0, 0, 21, 0, 0, 0),
        },
        StrToDateCase {
            input: " 2/Jun",
            format: "%d/%b/%Y",
            expect: types::FromDate(0, 6, 2, 0, 0, 0, 0),
        },
        StrToDateCase {
            input: " liter",
            format: "lit era l",
            expect: types::ZeroCoreTime,
        },
        StrToDateCase {
            input: "29/Feb/2020 12:34:56.",
            format: "%d/%b/%Y %H:%i:%s.%f",
            expect: types::FromDate(2020, 2, 29, 12, 34, 56, 0),
        },
        // AllowInvalidDate=true 时只检查月 1..12、日 1..31，保留 Go 对不存在日期的接受行为。
        StrToDateCase {
            input: "31/April/2016 12:34:56.",
            format: "%d/%M/%Y %H:%i:%s.%f",
            expect: types::FromDate(2016, 4, 31, 12, 34, 56, 0),
        },
        StrToDateCase {
            input: "29/Feb/2021 12:34:56.",
            format: "%d/%b/%Y %H:%i:%s.%f",
            expect: types::FromDate(2021, 2, 29, 12, 34, 56, 0),
        },
        StrToDateCase {
            input: "30/Feb/2016 12:34:56.1234",
            format: "%d/%b/%Y %H:%i:%S.%f",
            expect: types::FromDate(2016, 2, 30, 12, 34, 56, 123400),
        },
    ];

    for (i, tt) in tests.iter().enumerate() {
        typeCtx.flags.ignore_invalid_date = true;
        let mut tm = types::Time::default();
        assert!(
            tm.StrToDate(&typeCtx, tt.input, tt.format),
            "case {i}: input={} format={}",
            tt.input,
            tt.format
        );
        assert_eq!(
            tt.expect,
            tm.CoreTime(),
            "case {i}: input={} format={}",
            tt.input,
            tt.format
        );
    }

    // 关闭 IgnoreInvalidDate 后应拒绝的非法日期与格式错配
    let errTests = vec![
        StrToDateErrCase {
            input: "04/31/2004",
            format: "%m/%d/%Y",
        },
        StrToDateErrCase {
            input: "29/Feb/2021 12:34:56.",
            format: "%d/%b/%Y %H:%i:%s.%f",
        },
        StrToDateErrCase {
            input: "512 2021",
            format: "%m%d %Y",
        },
        StrToDateErrCase {
            input: "a09:30:17",
            format: "%h:%i:%s",
        },
        StrToDateErrCase {
            input: "12:43:24 a",
            format: "%r",
        },
        StrToDateErrCase {
            input: "23:60:12",
            format: "%T",
        },
        StrToDateErrCase {
            input: "18",
            format: "%l",
        },
        StrToDateErrCase {
            input: "00:21:22 AM",
            format: "%h:%i:%s %p",
        },
        StrToDateErrCase {
            input: "100/10/22",
            format: "%y/%m/%d",
        },
        StrToDateErrCase {
            input: "2010-11-12 11 am",
            format: "%Y-%m-%d %H %p",
        },
        StrToDateErrCase {
            input: "2010-11-12 13 am",
            format: "%Y-%m-%d %h %p",
        },
        StrToDateErrCase {
            input: "2010-11-12 0 am",
            format: "%Y-%m-%d %h %p",
        },
        StrToDateErrCase {
            input: "15 SEPTEMB 2001",
            format: "%d %M %Y",
        },
        StrToDateErrCase {
            input: "13:13:56 AM13/5/2019",
            format: "%r",
        },
        StrToDateErrCase {
            input: "00:13:56 AM13/05/2019",
            format: "%r",
        },
        StrToDateErrCase {
            input: "00:13:56 pM13/05/2019",
            format: "%r",
        },
        StrToDateErrCase {
            input: "11:13:56a",
            format: "%r",
        },
    ];
    for (i, tt) in errTests.iter().enumerate() {
        typeCtx.flags.ignore_invalid_date = false;
        let mut tm = types::Time::default();
        assert!(
            !tm.StrToDate(&typeCtx, tt.input, tt.format),
            "case {i}: input={} format={}",
            tt.input,
            tt.format
        );
    }
}

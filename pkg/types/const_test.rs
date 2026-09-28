// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// SQL Mode 与服务器状态标志解析的单元测试。
//
// 覆盖 `GetSQLMode` / `FormatSQLModeStr` 的合法与非法输入，
// 以及 `HasCursorExistsFlag` 对 ServerStatus 位标志的判定。

use parser_types::mysql;

/// 合法 SQL Mode 字符串可解析；含空格或未知模式名则失败。
#[test]
fn test_get_sql_mode() {
    // FormatSQLModeStr 会规范化多余逗号，再交给 GetSQLMode
    let positive_cases = [
        "NO_ZERO_DATE",
        ",,NO_ZERO_DATE",
        "NO_ZERO_DATE,NO_ZERO_IN_DATE",
        "",
        ", ",
        ",",
    ];
    for argument in positive_cases {
        assert!(mysql::GetSQLMode(&mysql::FormatSQLModeStr(argument)).is_ok());
    }

    // 逗号后带空格或未知模式名视为非法
    let negative_cases = [
        "NO_ZERO_DATE, NO_ZERO_IN_DATE",
        "NO_ZERO_DATE,adfadsdfasdfads",
        ", ,NO_ZERO_DATE",
        " ,",
    ];
    for argument in negative_cases {
        assert!(mysql::GetSQLMode(&mysql::FormatSQLModeStr(argument)).is_err());
    }
}

/// 校验各 SQL Mode 标志位（零日期、除零错误等）的组合解析结果。
#[test]
fn test_sql_mode() {
    let cases = [
        ("NO_ZERO_DATE", true, false, false),
        ("NO_ZERO_IN_DATE", false, true, false),
        ("ERROR_FOR_DIVISION_BY_ZERO", false, false, true),
        ("NO_ZERO_IN_DATE,NO_ZERO_DATE", true, true, false),
        ("NO_ZERO_DATE,NO_ZERO_IN_DATE", true, true, false),
        ("NO_ZERO_DATE,NO_ZERO_IN_DATE", true, true, false),
        (
            "NO_ZERO_DATE,NO_ZERO_IN_DATE,ERROR_FOR_DIVISION_BY_ZERO",
            true,
            true,
            true,
        ),
        (
            "NO_ZERO_IN_DATE,ERROR_FOR_DIVISION_BY_ZERO",
            false,
            true,
            true,
        ),
        ("", false, false, false),
    ];

    for (argument, no_zero_date, no_zero_in_date, division_by_zero) in cases {
        let sql_mode = mysql::GetSQLMode(argument).unwrap();
        assert_eq!(no_zero_date, sql_mode.HasNoZeroDateMode());
        assert_eq!(no_zero_in_date, sql_mode.HasNoZeroInDateMode());
        assert_eq!(division_by_zero, sql_mode.HasErrorForDivisionByZeroMode());
    }
}

/// 校验 ServerStatus 中 CursorExists 位是否被正确识别。
#[test]
fn test_server_status() {
    let cases = [
        (0_u16, false),
        (
            mysql::ServerStatusInTrans | mysql::ServerStatusNoBackslashEscaped,
            false,
        ),
        (mysql::ServerStatusCursorExists, true),
        (
            mysql::ServerStatusCursorExists | mysql::ServerStatusLastRowSend,
            true,
        ),
    ];

    for (status, cursor_exists) in cases {
        assert_eq!(cursor_exists, mysql::HasCursorExistsFlag(status));
    }
}

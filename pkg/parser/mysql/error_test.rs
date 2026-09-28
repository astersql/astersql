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

// 对照 Go error_test：覆盖 NewErr / NewErrf 四种构造入口的 Error() 输出。

// 本文件对照 pkg/parser/mysql/error_test.go 迁移，保留 Go 测试结构与行为。

// test_sql_error 对应 Go 的 TestSQLError。
// 它覆盖已知错误码、未知错误码、格式化错误和普通错误四种构造入口，确保 Error() 都能返回可展示文本。
use astersql_errors::ErrorArg;

use crate::errcode::{ErrNoDB, ErrWrongValueCountOnRow};
use crate::error::{NewErr, NewErrf};

#[test]
fn test_sql_error() {
    // 自定义模板 + 已知错误码：仍能产出非空展示文本。
    let e = NewErrf(ErrNoDB, "no db error", &[], vec![]);
    assert!(!e.Error().is_empty());

    // 未知错误码 0：走默认 SQLSTATE，自定义消息。
    let e = NewErrf(0, "customized error", &[], vec![]);
    assert!(!e.Error().is_empty());

    // 默认模板：从 MySQLErrName 取 “No database selected”。
    let e = NewErr(ErrNoDB, vec![]);
    assert!(!e.Error().is_empty());

    let e = NewErr(0, vec!["customized error".into(), "<nil>".into()]);
    // Go 这里传入 nil 作为第二个变参；Rust 用 Go fmt.Sprint 对 nil 的文本保留相同语义。
    assert!(!e.Error().is_empty());
}

/// Go 的 `args ...any` 保留参数类型，`%d` 必须能接收整数而不能把字符串伪装成整数。
#[test]
fn test_sql_error_preserves_format_argument_types() {
    let numeric = NewErr(ErrWrongValueCountOnRow, vec![ErrorArg::from(7_i32)]);
    assert_eq!(
        numeric.Message,
        "Column count doesn't match value count at row 7"
    );

    let wrong_type = NewErr(ErrWrongValueCountOnRow, vec![ErrorArg::from("7")]);
    assert_eq!(
        wrong_type.Message,
        "Column count doesn't match value count at row %!d(string=7)"
    );
}

// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// sqlescape 迁移补充单元测试。
//
// 对齐 Go 侧特殊字节转义、占位符与错误文案、数值/浮点/反射路径，
// 以及时间、JSON、二进制、切片与 Must/Writer 辅助函数语义。

use crate::{
    EscapeSQL, EscapeString, FormatSQL, GoTime, MustEscapeSQL, MustFormatSQL, ReflectedValue,
    SqlArg,
};

/// 验证 NUL/换行/回车/0x1a/引号/反斜杠及非 ASCII 的 EscapeString 结果。
#[test]
fn migration_escape_string_special_bytes() {
    assert_eq!(EscapeString("hello"), "hello");
    assert_eq!(
        EscapeString("\0\n\r\u{1a}'\"\\中文"),
        "\\0\\n\\r\\Z\\'\\\"\\\\中文"
    );
}

/// `%n`/`%?`/`%%` 成功路径，以及缺参、标识符类型错误、不支持参数的错误前缀。
#[test]
fn migration_formats_placeholders_and_errors_like_go() {
    assert_eq!(
        EscapeSQL(
            "use %n; select %?, %%, %v, %",
            &[
                SqlArg::String("db`name".into()),
                SqlArg::String("it's".into()),
            ],
        )
        .unwrap(),
        "use `db``name`; select 'it\\'s', %, %v, %"
    );

    let err = EscapeSQL("select %?, %?", &[SqlArg::Int(1)]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "missing arguments, need 2-th arg, but only got 1 args"
    );
    assert!(
        EscapeSQL("use %n", &[SqlArg::Int(1)])
            .unwrap_err()
            .to_string()
            .starts_with("expect a string identifier")
    );
    assert!(
        EscapeSQL("select %?", &[SqlArg::Unsupported("channel".into())])
            .unwrap_err()
            .to_string()
            .starts_with("unsupported 1-th argument")
    );
}

/// 覆盖有符号/无符号整数、布尔、浮点切片、科学计数与 Inf/NaN、反射路径。
#[test]
fn migration_formats_all_numeric_paths_with_go_semantics() {
    let args = [
        SqlArg::Int(-1),
        SqlArg::Int8(-2),
        SqlArg::Int16(-3),
        SqlArg::Int32(-4),
        SqlArg::Int64(-5),
        SqlArg::Uint(6),
        SqlArg::Uint8(7),
        SqlArg::Uint16(8),
        SqlArg::Uint32(9),
        SqlArg::Uint64(10),
        SqlArg::Bool(true),
        SqlArg::Bool(false),
    ];
    assert_eq!(
        EscapeSQL("%?,%?,%?,%?,%?,%?,%?,%?,%?,%?,%?,%?", &args).unwrap(),
        "-1,-2,-3,-4,-5,6,7,8,9,10,1,0"
    );
    assert_eq!(
        EscapeSQL(
            "%?;%?;%?;%?",
            &[
                SqlArg::Float32(1.0e20),
                SqlArg::Float64(1.0e20),
                SqlArg::Float32Slice(vec![33.1, 0.44]),
                SqlArg::Float64Slice(vec![55.2, 0.66]),
            ],
        )
        .unwrap(),
        "1e+20;1e+20;33.1,0.44;55.2,0.66"
    );
    assert_eq!(
        EscapeSQL(
            "%?,%?,%?,%?,%?,%?,%?",
            &[
                SqlArg::Float64(1e6),
                SqlArg::Float64(1e-4),
                SqlArg::Float64(1e-5),
                SqlArg::Float64(1e-7),
                SqlArg::Float64(f64::INFINITY),
                SqlArg::Float64(f64::NEG_INFINITY),
                SqlArg::Float64(f64::NAN),
            ],
        )
        .unwrap(),
        "1e+06,0.0001,1e-05,1e-07,+Inf,-Inf,NaN"
    );
    assert_eq!(
        EscapeSQL(
            "%?,%?",
            &[
                SqlArg::Reflected(ReflectedValue::Int(3)),
                SqlArg::Reflected(ReflectedValue::String("x".into())),
            ],
        )
        .unwrap(),
        "3,'x'"
    );
}

/// NULL、零日期、微秒时间、JSON、nil/空/含特殊字节二进制与字符串切片格式化。
#[test]
fn migration_formats_time_json_binary_and_slices() {
    let args = [
        SqlArg::Nil,
        SqlArg::Time(GoTime::zero()),
        SqlArg::Time(GoTime::from_components(2018, 1, 23, 4, 3, 5, 888_888_888).unwrap()),
        SqlArg::JsonRawMessage(br#"{"h": "hello"}"#.to_vec()),
        SqlArg::Bytes(None),
        SqlArg::Bytes(Some(Vec::new())),
        SqlArg::Bytes(Some(vec![0, b'\'', b'\\'])),
        SqlArg::StringSlice(vec!["a'b".into(), "c".into()]),
    ];
    assert_eq!(
        EscapeSQL("%?|%?|%?|%?|%?|%?|%?|%?", &args).unwrap(),
        "NULL|'0000-00-00'|'2018-01-23 04:03:05.888888'|'{\\\"h\\\": \\\"hello\\\"}'|NULL|_binary''|_binary'\\0\\'\\\\'|'a\\'b','c'"
    );
}

/// `FormatSQL`/`MustFormatSQL`/`MustEscapeSQL` 成功写出与字面量 `%%`。
#[test]
fn migration_writer_and_must_helpers_match_go() {
    let mut output = Vec::new();
    FormatSQL(&mut output, "select %?", &[SqlArg::String("ok".into())]).unwrap();
    assert_eq!(output, b"select 'ok'");

    let mut must_output = Vec::new();
    MustFormatSQL(&mut must_output, "select %?", &[SqlArg::Int(3)]);
    assert_eq!(must_output, b"select 3");
    assert_eq!(MustEscapeSQL("select %%", &[]), "select %");
}

/// 缺参时 `MustEscapeSQL` 以原 Go 错误文案 panic。
#[test]
#[should_panic(expected = "missing arguments, need 1-th arg, but only got 0 args")]
fn migration_must_escape_panics_with_original_error() {
    MustEscapeSQL("%?", &[]);
}

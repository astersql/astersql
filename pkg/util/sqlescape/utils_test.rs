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

// sqlescape utils 单元测试：缓冲扩容、反斜杠转义、EscapeSQL 表驱动与 Must 辅助。
//
// 对齐 Go 侧 reserveBuffer 容量公式、特殊字节转义、42 组 EscapeSQL 用例，
// 以及 MustEscapeSQL/MustFormatSQL 的 panic 文案与成功路径。

use chrono::NaiveDateTime;

use super::*;

/// 验证扩容后长度、容量公式 `len*2+appendSize` 以及前缀内容保留。
#[test]
fn test_reserve_buffer() {
    let res0 = reserveBuffer(Vec::new(), 0);
    assert!(res0.is_empty());

    let mut res1 = reserveBuffer(res0, 3);
    assert_eq!(res1.len(), 3);
    res1[1] = 3;

    let res2 = reserveBuffer(res1.clone(), 9);
    assert_eq!(res2.len(), 12);
    assert_eq!(res2.capacity(), 15);
    assert_eq!(&res2[..3], res1);
}

/// 逐用例校验 `escapeBytesBackslash` 与 `escapeStringBackslash` 结果一致。
#[test]
fn test_escape_backslash() {
    let tests: &[(&str, &[u8], &[u8])] = &[
        ("normal", b"hello", b"hello"),
        ("0", b"he\0lo", b"he\\0lo"),
        ("break line", b"he\nlo", b"he\\nlo"),
        ("carry", b"he\rlo", b"he\\rlo"),
        ("substitute", b"he\x1alo", b"he\\Zlo"),
        ("single quote", b"he'lo", b"he\\'lo"),
        ("double quote", b"he\"lo", b"he\\\"lo"),
        ("back slash", b"he\\lo", b"he\\\\lo"),
        ("double escape", b"he\0lo\"", b"he\\0lo\\\""),
        ("chinese", "中文?".as_bytes(), "中文?".as_bytes()),
    ];

    for (name, input, output) in tests {
        assert_eq!(escapeBytesBackslash(Vec::new(), input), *output, "{name}");
        assert_eq!(
            escapeStringBackslash(Vec::new(), std::str::from_utf8(input).unwrap()),
            *output,
            "{name}"
        );
    }
}

/// EscapeSQL 表驱动用例：成功时的期望输出，或失败时的错误前缀。
struct EscapeSqlCase {
    name: &'static str,
    input: &'static str,
    params: Vec<SqlArg>,
    output: &'static str,
    error_prefix: &'static str,
}

impl EscapeSqlCase {
    /// 构造期望成功的用例。
    fn ok(
        name: &'static str,
        input: &'static str,
        params: Vec<SqlArg>,
        output: &'static str,
    ) -> Self {
        Self {
            name,
            input,
            params,
            output,
            error_prefix: "",
        }
    }

    /// 构造期望失败且错误以 `error_prefix` 开头的用例。
    fn err(
        name: &'static str,
        input: &'static str,
        params: Vec<SqlArg>,
        error_prefix: &'static str,
    ) -> Self {
        Self {
            name,
            input,
            params,
            output: "",
            error_prefix,
        }
    }
}

/// 对 internal/public/writer 三路径跑 42 组 EscapeSQL 成功与错误用例。
#[test]
fn test_escape_sql() {
    let time2 = NaiveDateTime::parse_from_str("2018-01-23 04:03:05", "%Y-%m-%d %H:%M:%S").unwrap();
    let time3 = NaiveDateTime::parse_from_str("1970-01-01 00:00:00.888888", "%Y-%m-%d %H:%M:%S%.f")
        .unwrap();
    let tests = vec![
        EscapeSqlCase::ok("normal 1", "select * from 1", vec![], "select * from 1"),
        EscapeSqlCase::ok(
            "normal 2",
            "WHERE source != 'builtin'",
            vec![],
            "WHERE source != 'builtin'",
        ),
        EscapeSqlCase::ok(
            "discard extra arguments",
            "select * from 1",
            vec![SqlArg::Int(4), SqlArg::Int(5), SqlArg::String("rt".into())],
            "select * from 1",
        ),
        EscapeSqlCase::err(
            "%? missing arguments",
            "select %? from %?",
            vec![SqlArg::Int(4)],
            "missing arguments",
        ),
        EscapeSqlCase::ok("nil", "select %?", vec![SqlArg::Nil], "select NULL"),
        EscapeSqlCase::ok("int", "select %?", vec![SqlArg::Int(3)], "select 3"),
        EscapeSqlCase::ok("int8", "select %?", vec![SqlArg::Int8(4)], "select 4"),
        EscapeSqlCase::ok("int16", "select %?", vec![SqlArg::Int16(5)], "select 5"),
        EscapeSqlCase::ok("int32", "select %?", vec![SqlArg::Int32(6)], "select 6"),
        EscapeSqlCase::ok("int64", "select %?", vec![SqlArg::Int64(7)], "select 7"),
        EscapeSqlCase::ok("uint", "select %?", vec![SqlArg::Uint(8)], "select 8"),
        EscapeSqlCase::ok("uint8", "select %?", vec![SqlArg::Uint8(9)], "select 9"),
        EscapeSqlCase::ok("uint16", "select %?", vec![SqlArg::Uint16(10)], "select 10"),
        EscapeSqlCase::ok("uint32", "select %?", vec![SqlArg::Uint32(11)], "select 11"),
        EscapeSqlCase::ok("uint64", "select %?", vec![SqlArg::Uint64(12)], "select 12"),
        EscapeSqlCase::ok(
            "float32",
            "select %?",
            vec![SqlArg::Float32(0.13)],
            "select 0.13",
        ),
        EscapeSqlCase::ok(
            "float64",
            "select %?",
            vec![SqlArg::Float64(0.14)],
            "select 0.14",
        ),
        EscapeSqlCase::ok("bool on", "select %?", vec![SqlArg::Bool(true)], "select 1"),
        EscapeSqlCase::ok(
            "bool off",
            "select %?",
            vec![SqlArg::Bool(false)],
            "select 0",
        ),
        EscapeSqlCase::ok(
            "time 0",
            "select %?",
            vec![SqlArg::Time(GoTime::zero())],
            "select '0000-00-00'",
        ),
        EscapeSqlCase::ok(
            "time 1",
            "select %?",
            vec![SqlArg::Time(
                GoTime::from_components(2019, 1, 1, 0, 0, 0, 0).unwrap(),
            )],
            "select '2019-01-01 00:00:00'",
        ),
        EscapeSqlCase::ok(
            "time 2",
            "select %?",
            vec![SqlArg::Time(GoTime::from_naive(time2))],
            "select '2018-01-23 04:03:05'",
        ),
        EscapeSqlCase::ok(
            "time 3",
            "select %?",
            vec![SqlArg::Time(GoTime::from_naive(time3))],
            "select '1970-01-01 00:00:00.888888'",
        ),
        EscapeSqlCase::ok(
            "empty byte slice1",
            "select %?",
            vec![SqlArg::Bytes(None)],
            "select NULL",
        ),
        EscapeSqlCase::ok(
            "empty byte slice2",
            "select %?",
            vec![SqlArg::Bytes(Some(vec![]))],
            "select _binary''",
        ),
        EscapeSqlCase::ok(
            "byte slice",
            "select %?",
            vec![SqlArg::Bytes(Some(vec![2, 3]))],
            "select _binary'\x02\x03'",
        ),
        EscapeSqlCase::ok(
            "string",
            "select %?",
            vec![SqlArg::String("33".into())],
            "select '33'",
        ),
        EscapeSqlCase::ok(
            "string slice",
            "select %?",
            vec![SqlArg::StringSlice(vec!["33".into(), "44".into()])],
            "select '33','44'",
        ),
        EscapeSqlCase::ok(
            "raw json",
            "select %?",
            vec![SqlArg::JsonRawMessage(br#"{"h": "hello"}"#.to_vec())],
            "select '{\\\"h\\\": \\\"hello\\\"}'",
        ),
        EscapeSqlCase::err(
            "unsupported args",
            "select %?",
            vec![SqlArg::Unsupported("channel".into())],
            "unsupported 1-th argument",
        ),
        EscapeSqlCase::ok(
            "mixed arguments",
            "select %?, %?, %?",
            vec![
                SqlArg::String("33".into()),
                SqlArg::Int(44),
                SqlArg::Time(GoTime::zero()),
            ],
            "select '33', 44, '0000-00-00'",
        ),
        EscapeSqlCase::ok(
            "simple injection",
            "select %?",
            vec![SqlArg::String("0; drop database".into())],
            "select '0; drop database'",
        ),
        EscapeSqlCase::err(
            "identifier, wrong arg",
            "use %n",
            vec![SqlArg::Int(3)],
            "expect a string identifier",
        ),
        EscapeSqlCase::ok(
            "identifier",
            "use %n",
            vec![SqlArg::String("table`".into())],
            "use `table```",
        ),
        EscapeSqlCase::err(
            "%n missing arguments",
            "use %n",
            vec![],
            "missing arguments",
        ),
        EscapeSqlCase::ok(
            "% escape",
            "select * from t where val = '%%?'",
            vec![],
            "select * from t where val = '%?'",
        ),
        EscapeSqlCase::ok("unknown specifier", "%v", vec![], "%v"),
        EscapeSqlCase::ok("truncated specifier ", "rv %", vec![], "rv %"),
        EscapeSqlCase::ok(
            "float32 slice",
            "select %?",
            vec![SqlArg::Float32Slice(vec![33.1, 0.44])],
            "select 33.1,0.44",
        ),
        EscapeSqlCase::ok(
            "float64 slice",
            "select %?",
            vec![SqlArg::Float64Slice(vec![55.2, 0.66])],
            "select 55.2,0.66",
        ),
        EscapeSqlCase::ok(
            "myInt",
            "select %?",
            vec![SqlArg::Reflected(ReflectedValue::Int(3))],
            "select 3",
        ),
        EscapeSqlCase::ok(
            "myStr",
            "select %?",
            vec![SqlArg::Reflected(ReflectedValue::String("3".into()))],
            "select '3'",
        ),
    ];

    assert_eq!(tests.len(), 42);
    for case in tests {
        let internal =
            escapeSQL(case.input, &case.params).map(|bytes| String::from_utf8(bytes).unwrap());
        let public = EscapeSQL(case.input, &case.params);
        let mut formatted = Vec::new();
        let writer = FormatSQL(&mut formatted, case.input, &case.params)
            .map(|()| String::from_utf8(formatted).unwrap());

        if case.error_prefix.is_empty() {
            assert_eq!(internal.unwrap(), case.output, "{} internal", case.name);
            assert_eq!(public.unwrap(), case.output, "{} public", case.name);
            assert_eq!(writer.unwrap(), case.output, "{} writer", case.name);
        } else {
            for error in [
                internal.unwrap_err().to_string(),
                public.unwrap_err().to_string(),
                writer.unwrap_err().to_string(),
            ] {
                assert!(
                    error.starts_with(case.error_prefix),
                    "{}: {error}",
                    case.name
                );
            }
        }
    }
}

/// Must 辅助在缺参时 panic，文案与 Go 一致；无占位符时成功写出。
#[test]
fn test_must_utils() {
    let panic = std::panic::catch_unwind(|| MustEscapeSQL("%?", &[])).unwrap_err();
    assert_eq!(
        panic_message(panic),
        "missing arguments, need 1-th arg, but only got 0 args"
    );

    let panic = std::panic::catch_unwind(|| {
        let mut sql = Vec::new();
        MustFormatSQL(&mut sql, "%?", &[]);
    })
    .unwrap_err();
    assert_eq!(
        panic_message(panic),
        "missing arguments, need 1-th arg, but only got 0 args"
    );

    let mut sql = Vec::new();
    MustFormatSQL(&mut sql, "t", &[]);
    assert_eq!(sql, b"t");
    assert_eq!(MustEscapeSQL("tt", &[]), "tt");
}

/// 从 `catch_unwind` 的 payload 取出 panic 字符串消息。
fn panic_message(value: Box<dyn std::any::Any + Send>) -> String {
    match value.downcast::<String>() {
        Ok(message) => *message,
        Err(value) => value
            .downcast::<&str>()
            .map(|message| (*message).to_owned())
            .unwrap(),
    }
}

/// 普通文本、含单引号与含双引号的 EscapeString 结果。
#[test]
fn test_escape_string() {
    for (input, output) in [
        ("testData", "testData"),
        ("it's all good", "it\\'s all good"),
        (r#"+ -><()~*:""&|"#, r#"+ -><()~*:\"\"&|"#),
    ] {
        assert_eq!(EscapeString(input), output);
    }
}

// Rust stable 没有 Go testing.B 的同形接口；保留四条 benchmark 调用路径供外部 harness 使用。
/// 字符串参数路径的基准入口（无内建 harness，仅保留调用点）。
#[allow(dead_code)]
fn benchmark_escape_string() {
    let _ = escapeSQL("select %?", &[SqlArg::String("3".into())]);
}
/// 反射字符串慢路径基准入口。
#[allow(dead_code)]
fn benchmark_underlying_string() {
    let _ = escapeSQL(
        "select %?",
        &[SqlArg::Reflected(ReflectedValue::String("3".into()))],
    );
}
/// 整数参数路径的基准入口。
#[allow(dead_code)]
fn benchmark_escape_int() {
    let _ = escapeSQL("select %?", &[SqlArg::Int(3)]);
}
/// 反射整数慢路径基准入口。
#[allow(dead_code)]
fn benchmark_underlying_int() {
    let _ = escapeSQL("select %?", &[SqlArg::Reflected(ReflectedValue::Int(3))]);
}

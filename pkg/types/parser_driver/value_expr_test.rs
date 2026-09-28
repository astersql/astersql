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

// ValueExpr 的 Restore/Format 单元测试，对齐 Go `TestValueExpr*`。
//
// 表驱动覆盖 NULL、整数、浮点、字符串转义、二进制字面量、Decimal、
// Duration 与 Time 等 Datum 种类的 SQL 字面量输出。

use parser_driver::{ValueExpr, format};
use types_decimal::mydecimal::NewDecFromInt;

/// 用默认 Restore 标志把 Datum 还原为 SQL 字符串。
fn restore(datum: types::Datum) -> String {
    let mut expression = ValueExpr::default();
    expression.Datum = datum;
    expression
        .RestoreToString(format::DefaultRestoreFlags)
        .expect("Go restore cases must not fail")
}

/// 调用 Format 写入缓冲区并转为 UTF-8 字符串。
fn formatted(datum: types::Datum) -> String {
    let mut expression = ValueExpr::default();
    expression.Datum = datum;
    let mut output = Vec::new();
    expression.Format(&mut output);
    String::from_utf8(output).expect("formatted SQL is UTF-8")
}

#[test]
#[allow(non_snake_case)]
/// 校验 RestoreToString 与 Go 用例表一致。
fn TestValueExprRestore() {
    testsetup::SetupForCommonTest();
    let bytes = b"test `s't\"r.".to_vec();
    let cases = vec![
        (types::Datum::default(), "NULL"),
        (types::NewIntDatum(1), "1"),
        (types::NewIntDatum(-1), "-1"),
        (types::NewUintDatum(1), "1"),
        (types::NewFloat32Datum(1.1), "1.1e+00"),
        (types::NewFloat64Datum(1.1), "1.1e+00"),
        (
            types::NewStringDatum("test `s't\"r.".to_owned()),
            "'test `s''t\"r.'",
        ),
        (types::NewBytesDatum(bytes.clone()), "'test `s''t\"r.'"),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(bytes)),
            "b'11101000110010101110011011101000010000001100000011100110010011101110100001000100111001000101110'",
        ),
        (types::NewDecimalDatum(NewDecFromInt(321)), "321"),
        (types::NewDurationDatum(types::ZeroDuration), "'00:00:00'"),
        (
            types::NewTimeDatum(types::Time::default()),
            "'0000-00-00 00:00:00'",
        ),
        (types::NewStringDatum("\\".to_owned()), "'\\\\'"),
    ];

    assert_eq!(cases.len(), 13);
    for (datum, expected) in cases {
        assert_eq!(restore(datum), expected);
    }
}

#[test]
#[allow(non_snake_case)]
/// 校验 Format 紧凑输出与 Go 用例表一致。
fn TestValueExprFormat() {
    testsetup::SetupForCommonTest();
    let bytes = b"test `s't\"r.".to_vec();
    let cases = vec![
        (types::Datum::default(), "NULL"),
        (types::NewIntDatum(1), "1"),
        (types::NewIntDatum(-1), "-1"),
        (types::NewUintDatum(1), "1"),
        (types::NewFloat32Datum(1.1), "1.1e+00"),
        (types::NewFloat64Datum(1.1), "1.1e+00"),
        (
            types::NewStringDatum("test `s't\"r.".to_owned()),
            "'test `s''t\"r.'",
        ),
        (types::NewBytesDatum(bytes.clone()), "'test `s''t\"r.'"),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(bytes)),
            "b'11101000110010101110011011101000010000001100000011100110010011101110100001000100111001000101110'",
        ),
        (types::NewDecimalDatum(NewDecFromInt(321)), "321"),
        (types::NewStringDatum("\\".to_owned()), "'\\\\'"),
        (types::NewStringDatum("''".to_owned()), "''''''"),
        (
            types::NewStringDatum("\\''\t\n".to_owned()),
            "'\\\\''''\t\n'",
        ),
    ];

    assert_eq!(cases.len(), 13);
    for (datum, expected) in cases {
        assert_eq!(formatted(datum), expected);
    }
}

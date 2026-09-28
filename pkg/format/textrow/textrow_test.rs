// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// FormatValueText / AppendFormatFloat 的单元测试。
//
// 对照 Go `server/internal/column` 中 TestDumpTextValue 等用例，覆盖整数、
// 浮点精度、字符串/GBK、时态、Decimal、Year、Enum/Set/JSON 以及无效类型与浮点表。

use super::{
    AppendFormatFloat, ColumnInfo, ErrInvalidType, FormatValueText, NewResultEncoder,
    ResultEncoder, charset, chunk, mysql, types,
};

// appendValue 对应 Go 测试里的小辅助函数：把单个 Datum 包装成一列 Row 后调用 FormatValueText。
fn appendValue(col: ColumnInfo, enc: &mut ResultEncoder, d: types::Datum) -> Vec<u8> {
    let row = chunk::MutRowFromDatums(vec![d]);
    let got = FormatValueText(&row.ToRow(), 0, &col, enc).expect("FormatValueText should succeed");
    got
}

// TestFormatValueText 验证 DumpTextRow length-encoding 内部的文本值字节。
// 期望值沿用 Go 中 server/internal/column TestDumpTextValue 已证明的输出。
#[test]
fn TestFormatValueText() {
    let mut utf8 = NewResultEncoder(charset::CharsetUTF8MB4);

    // signed / unsigned integer：同一个 MySQL Longlong 类型通过 unsigned flag 区分输出路径。
    assert_eq!(
        "10",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeLonglong,
                Decimal: mysql::NotFixedDec as u8,
                ..Default::default()
            },
            &mut utf8,
            types::NewIntDatum(10),
        )),
    );
    assert_eq!(
        "11",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeLonglong,
                Flag: mysql::UnsignedFlag as u16,
                ..Default::default()
            },
            &mut utf8,
            types::NewUintDatum(11),
        )),
    );

    // float / double precision 只在 Table 为空时覆盖 strconv 的默认精度。
    let f32 = types::NewFloat32Datum(1.2);
    assert_eq!(
        "1.2",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeFloat,
                Decimal: 1,
                ..Default::default()
            },
            &mut utf8,
            f32.clone(),
        )),
    );
    assert_eq!(
        "1.20",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeFloat,
                Decimal: 2,
                ..Default::default()
            },
            &mut utf8,
            f32,
        )),
    );
    let f64 = types::NewFloat64Datum(2.2);
    assert_eq!(
        "2.2",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeDouble,
                Decimal: 1,
                ..Default::default()
            },
            &mut utf8,
            f64.clone(),
        )),
    );
    assert_eq!(
        "2.20",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeDouble,
                Decimal: 2,
                ..Default::default()
            },
            &mut utf8,
            f64.clone(),
        )),
    );
    // Table 非空时保留完整精度，避免覆盖表列实际格式化行为。
    assert_eq!(
        "2.2",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeDouble,
                Decimal: 2,
                Table: "t".to_owned(),
                ..Default::default()
            },
            &mut utf8,
            f64,
        )),
    );

    // strings / blobs：Blob 和 Varchar 都走字符串字节输出，但 Datum 构造方式不同。
    assert_eq!(
        "foo",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeBlob,
                ..Default::default()
            },
            &mut utf8,
            types::NewBytesDatum(b"foo".to_vec()),
        )),
    );
    assert_eq!(
        "bar",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeVarchar,
                ..Default::default()
            },
            &mut utf8,
            types::NewStringDatum("bar".to_owned()),
        )),
    );

    // result encoder 负责字符集转换，这里沿用 Go 用例验证“一”到 GBK 字节。
    let mut gbk = NewResultEncoder("gbk");
    assert_eq!(
        vec![0xd2, 0xbb],
        appendValue(
            ColumnInfo {
                Type: mysql::TypeVarchar,
                ..Default::default()
            },
            &mut gbk,
            types::NewStringDatum("一".to_owned()),
        ),
    );

    // datetime / duration / decimal：Go 测试使用洛杉矶时区和严格类型上下文解析。
    let type_ctx = types::BasicTimeContext {
        flags: types::TimeFlags {
            ignore_zero_in_date: true,
            ..Default::default()
        },
        location: chrono_tz::America::Los_Angeles,
    };
    let tm = types::ParseTime(
        &type_ctx,
        "2017-01-05 23:59:59.575601",
        mysql::TypeDatetime,
        0,
    )
    .expect("datetime should parse");
    let mut d = types::Datum::default();
    d.SetMysqlTime(tm);
    assert_eq!(
        "2017-01-06 00:00:00",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeDatetime,
                ..Default::default()
            },
            &mut utf8,
            d.clone(),
        )),
    );

    let (duration, _) =
        types::ParseDuration(&type_ctx, "11:30:45", 0).expect("duration should parse");
    d.SetMysqlDuration(duration);
    assert_eq!(
        "11:30:45",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeDuration,
                Decimal: 0,
                ..Default::default()
            },
            &mut utf8,
            d.clone(),
        )),
    );

    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"1.23").expect("decimal should parse");
    d.SetMysqlDecimal(decimal);
    assert_eq!(
        "1.23",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeNewDecimal,
                ..Default::default()
            },
            &mut utf8,
            d.clone(),
        )),
    );

    // year 保持四位零值，不把 0 简化成普通整数文本。
    assert_eq!(
        "0000",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeYear,
                ..Default::default()
            },
            &mut utf8,
            types::NewIntDatum(0),
        )),
    );
    assert_eq!(
        "1984",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeYear,
                ..Default::default()
            },
            &mut utf8,
            types::NewIntDatum(1984),
        )),
    );

    // enum / set / json：保留 Go 中构造 Datum 的顺序，分别验证名称和规范 JSON 字符串。
    assert_eq!(
        "ename",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeEnum,
                ..Default::default()
            },
            &mut utf8,
            types::NewMysqlEnumDatum(types::Enum {
                Name: "ename".to_owned(),
                Value: 0
            }),
        )),
    );
    let set = types::NewMysqlSetDatum(
        types::Set {
            Name: "sname".to_owned(),
            Value: 0,
        },
        mysql::DefaultCollationName.to_owned(),
    );
    assert_eq!(
        "sname",
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeSet,
                ..Default::default()
            },
            &mut utf8,
            set,
        )),
    );
    let binary_json =
        types::ParseBinaryJSONFromString(r#"{"a": 1, "b": 2}"#).expect("json should parse");
    let js = types::NewJSONDatum(binary_json);
    assert_eq!(
        r#"{"a": 1, "b": 2}"#,
        String::from_utf8_lossy(&appendValue(
            ColumnInfo {
                Type: mysql::TypeJSON,
                ..Default::default()
            },
            &mut utf8,
            js,
        )),
    );
}

// TestFormatValueTextInvalidType 对应 Go 的错误分支：Geometry 不支持文本格式化。
#[test]
fn TestFormatValueTextInvalidType() {
    let mut utf8 = NewResultEncoder(charset::CharsetUTF8MB4);
    let row = chunk::MutRowFromDatums(vec![types::NewIntDatum(1)]);
    let err = FormatValueText(
        &row.ToRow(),
        0,
        &ColumnInfo {
            Type: mysql::TypeGeometry,
            ..Default::default()
        },
        &mut utf8,
    )
    .unwrap_err();
    assert_eq!(ErrInvalidType, err);
}

// TestAppendFormatFloat 保留 Go 的表驱动用例，覆盖科学计数法、定点精度、32/64 位和无穷值。
#[test]
fn TestAppendFormatFloat() {
    let inf_val = f64::INFINITY;
    struct Case {
        f_val: f64,
        out: &'static str,
        prec: i32,
        bit_size: i32,
    }
    let tests = vec![
        Case {
            f_val: 99999999999999999999.0,
            out: "1e20",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 1e15,
            out: "1e15",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 9e14,
            out: "900000000000000",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: -9999999999999999.0,
            out: "-1e16",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 999999999999999.0,
            out: "999999999999999",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 0.000000000000001,
            out: "0.000000000000001",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 0.0000000000000009,
            out: "9e-16",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: -0.0000000000000009,
            out: "-9e-16",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 0.11111,
            out: "0.111",
            prec: 3,
            bit_size: 64,
        },
        Case {
            f_val: 0.11111,
            out: "0.111",
            prec: 3,
            bit_size: 64,
        },
        Case {
            f_val: 0.1111111111111111111,
            out: "0.11111111",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: 0.1111111111111111111,
            out: "0.1111111111111111",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 0.0000000000000009,
            out: "9e-16",
            prec: 3,
            bit_size: 64,
        },
        Case {
            f_val: 0.0,
            out: "0",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: -340282346638528860000000000000000000000.0,
            out: "-3.40282e38",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: -34028236.0,
            out: "-34028236.00",
            prec: 2,
            bit_size: 32,
        },
        Case {
            f_val: -17976921.34,
            out: "-17976921.34",
            prec: 2,
            bit_size: 64,
        },
        Case {
            f_val: -3.402823466e38,
            out: "-3.40282e38",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: -1.7976931348623157e308,
            out: "-1.7976931348623157e308",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 10.0e20,
            out: "1e21",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: 1e20,
            out: "1e20",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: 10.0,
            out: "10",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: 999999986991104.0,
            out: "1e15",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: 1e15,
            out: "1e15",
            prec: -1,
            bit_size: 32,
        },
        Case {
            f_val: inf_val,
            out: "0",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: -inf_val,
            out: "0",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 1e14,
            out: "100000000000000",
            prec: -1,
            bit_size: 64,
        },
        Case {
            f_val: 1e308,
            out: "1e308",
            prec: -1,
            bit_size: 64,
        },
    ];

    for tc in tests {
        // Go 把 nil 作为目标缓冲传入 AppendFormatFloat；Rust 用空 Vec 表达相同追加起点。
        let got = AppendFormatFloat(Vec::<u8>::new(), tc.f_val, tc.prec, tc.bit_size);
        assert_eq!(tc.out, String::from_utf8_lossy(&got));
    }
}

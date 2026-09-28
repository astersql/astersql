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

// Datum 迁移兼容性回归测试。
//
// 覆盖 Rust 实现与 Go 版本容易产生偏差的边界语义：十进制转无符号整数时的舍入与截断值、
// 字符串 Datum 对任意原始字节的保留、JSON 转换失败时携带的最佳努力结果，以及错误类型身份。

use super::*;

/// 按生产代码的解析路径构造十进制数，便于集中描述转换边界用例。
fn decimal(value: &str) -> MyDecimal {
    let mut decimal = MyDecimal::default();
    decimal.FromString(value.as_bytes()).unwrap();
    decimal
}

/// 与 Go 版本一致：依据首个小数位舍入，而不是直接截断小数部分。
#[test]
fn decimal_to_uint_rounds_and_preserves_the_upper_bound() {
    assert_eq!(
        2,
        ConvertDecimalToUint(&decimal("1.5"), u64::MAX, mysql::TypeLonglong).unwrap()
    );
    assert_eq!(
        1,
        ConvertDecimalToUint(&decimal("1.499"), u64::MAX, mysql::TypeLonglong).unwrap()
    );
    assert_eq!(
        10,
        ConvertDecimalToUint(&decimal("9.5"), 10, mysql::TypeTiny).unwrap()
    );
}

/// 溢出时既要返回错误，也要在错误中保留裁剪后的类型上界。
#[test]
fn decimal_to_uint_overflow_preserves_the_clipped_value() {
    let error = ConvertDecimalToUint(&decimal("10.5"), 10, mysql::TypeTiny).unwrap_err();
    assert_eq!(error.value, 10);
    assert!(error.to_string().contains("overflows"));
}

/// Go 字符串可保留任意字节，因此智能格式化必须检查 Datum 原始字节，
/// 不能基于有损 UTF-8 替换后的文本判断输出形式。
#[test]
fn smart_string_formatting_hex_encodes_invalid_utf8_bytes() {
    let mut datum = Datum::default();
    datum.SetBytesAsString(
        vec![0x61, 0x62, 0x63, 0xc3],
        charset::CollationBin.to_owned(),
        4,
    );

    assert_eq!(DatumsToStringSmart(&[datum], true).unwrap(), "0x616263C3");
}

/// 二进制排序规则直接比较 Go 字符串的原始字节，包括无效 UTF-8 字节。
#[test]
fn binary_string_comparison_preserves_invalid_utf8_bytes() {
    let mut left = Datum::default();
    left.SetBytesAsString(vec![0xff], charset::CollationBin.to_owned(), 1);
    let mut right = Datum::default();
    right.SetBytesAsString(vec![0xfe], charset::CollationBin.to_owned(), 1);
    let collator = collate::GetBinaryCollator();

    assert_eq!(
        left.Compare(
            DefaultStmtNoWarningContext.clone(),
            &right,
            collator.as_ref(),
        )
        .unwrap(),
        1
    );
}

/// JSON 标量转换发生截断错误时仍保留 Go 版本的最佳努力值，
/// 浮点数转整数也采用相同的就近偶数舍入规则。
#[test]
fn json_casts_preserve_best_effort_values() {
    let float_error = ConvertJSONToFloat(
        DefaultStmtNoWarningContext.clone(),
        CreateBinaryJSON("123.456hello"),
    )
    .unwrap_err();
    assert_eq!(float_error.value, 123.456);

    let int_error = ConvertJSONToInt64(
        DefaultStmtNoWarningContext.clone(),
        CreateBinaryJSON("123hello"),
        false,
    )
    .unwrap_err();
    assert_eq!(int_error.value, 123);

    assert_eq!(
        ConvertJSONToInt64(
            DefaultStmtNoWarningContext.clone(),
            CreateBinaryJSON(4.5_f64),
            false,
        )
        .unwrap(),
        4
    );

    let decimal_error =
        ConvertJSONToDecimal(DefaultStmtNoWarningContext.clone(), CreateBinaryJSON(()))
            .unwrap_err();
    assert_eq!(decimal_error.value.String(), "0");
}

/// ErrorWithValue 转换必须保留规范化后的 TiDB 错误身份，而不只是渲染后的消息文本。
#[test]
fn value_error_bridge_preserves_typed_error_identity() {
    let shared = ErrOverflow.GenWithStackByArgs(&["BIGINT".into(), "1".into()]);
    let wrapped: errors::Error = ErrorWithValue::new(0_u64, shared).into();

    assert!(wrapped.Equal(ErrOverflow.as_ref()));
    assert!(terror::ErrorEqual(&wrapped, ErrOverflow.as_ref()));
}

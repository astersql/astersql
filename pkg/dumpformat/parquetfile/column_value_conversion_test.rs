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

// Parquet 原始列值转换的边界与兼容性测试。
//
// 覆盖 DECIMAL 定标与二补码编码、原始字节按物理/逻辑类型解析、
// 可空时间戳的零日期降级，以及各物理类型写入对应列缓冲的分派。
// 用例重点锁定 Go 实现保留的截断、溢出和错误分支语义。

use crate::column_buffer::ColumnBuffer;
use crate::column_value::{
    append_column_value, parse_decimal_to_scaled_integer, parse_raw_column_value,
    to_fixed_len_two_complement,
};
use crate::{Column, ColumnInfo, ColumnType, ColumnValue, LogicalType, PhysicalType, TimeUnit};

/// 构造仅包含转换所需元数据的测试列；DECIMAL 的精度与小数位会同步到列信息。
fn column(physical: PhysicalType, logical: LogicalType, width: i32, nullable: bool) -> Column {
    let (precision, scale) = match logical {
        LogicalType::Decimal { precision, scale } => (precision, scale),
        _ => (0, 0),
    };
    Column {
        info: ColumnInfo {
            name: "c".into(),
            database_type_name: String::new(),
            nullable,
            precision,
            scale,
        },
        column_type: ColumnType {
            physical,
            logical,
            type_length: width,
            precision,
            scale,
        },
        allows_null_encoding: nullable,
        timestamp_unit: TimeUnit::Micros,
    }
}

#[test]
fn parse_decimal_to_scaled_integer_truncates_toward_zero() {
    // 多余小数位必须向零截断；省略整数部分时仍按 scale 在右侧补零。
    assert_eq!(parse_decimal_to_scaled_integer("12.349", 2).unwrap(), 1234);
    assert_eq!(
        parse_decimal_to_scaled_integer("-12.349", 2).unwrap(),
        -1234
    );
    assert_eq!(parse_decimal_to_scaled_integer(".5", 3).unwrap(), 500);
    assert!(parse_decimal_to_scaled_integer("not-decimal", 2).is_err());
    assert!(
        parse_decimal_to_scaled_integer("1", -1)
            .unwrap_err()
            .to_string()
            .contains("invalid decimal scale")
    );
}

#[test]
fn fixed_width_twos_complement_checks_boundaries_and_overflow() {
    // 正数左侧补零、负数保留符号扩展；超出有符号宽度或零宽度均应拒绝。
    assert_eq!(
        to_fixed_len_two_complement(255, 2).unwrap(),
        vec![0x00, 0xff]
    );
    assert_eq!(
        to_fixed_len_two_complement(-1, 2).unwrap(),
        vec![0xff, 0xff]
    );
    assert!(to_fixed_len_two_complement(128, 1).is_err());
    assert!(to_fixed_len_two_complement(-129, 1).is_err());
    assert!(to_fixed_len_two_complement(0, 0).is_err());
}

#[test]
fn parse_raw_column_value_covers_success_and_error_branches() {
    let boolean = column(PhysicalType::Boolean, LogicalType::None, -1, false);
    assert_eq!(
        parse_raw_column_value(b"true", &boolean).unwrap(),
        (Some(ColumnValue::Bool(true)), false)
    );
    assert!(parse_raw_column_value(b"bad-bool", &boolean).is_err());

    let decimal = column(
        PhysicalType::Int32,
        LogicalType::Decimal {
            precision: 9,
            scale: 2,
        },
        -1,
        false,
    );
    assert_eq!(
        parse_raw_column_value(b"12.34", &decimal).unwrap().0,
        Some(ColumnValue::Int32(1234))
    );
    assert!(parse_raw_column_value(b"21474836.48", &decimal).is_err());

    let timestamp = column(
        PhysicalType::Int64,
        LogicalType::Timestamp {
            adjusted_to_utc: false,
            unit: TimeUnit::Micros,
        },
        -1,
        true,
    );
    assert_eq!(
        parse_raw_column_value(b"2024-01-02 03:04:05.123456", &timestamp)
            .unwrap()
            .0,
        Some(ColumnValue::Int64(1_704_164_645_123_456))
    );
    assert_eq!(
        parse_raw_column_value(b"2024-01-02 03:04:05", &timestamp)
            .unwrap()
            .0,
        Some(ColumnValue::Int64(1_704_164_645_000_000))
    );
    assert_eq!(
        parse_raw_column_value(b"2024-01-02 03:04:05.1", &timestamp)
            .unwrap()
            .0,
        Some(ColumnValue::Int64(1_704_164_645_100_000))
    );
    let mut millis_timestamp = timestamp.clone();
    millis_timestamp.timestamp_unit = TimeUnit::Millis;
    assert_eq!(
        parse_raw_column_value(b"2024-01-02 03:04:05.123", &millis_timestamp)
            .unwrap()
            .0,
        Some(ColumnValue::Int64(1_704_164_645_123))
    );
    // 允许空值编码时，MySQL 零日期按不可解析时间降级为 NULL。
    assert_eq!(
        parse_raw_column_value(b"0000-00-00 00:00:00", &timestamp).unwrap(),
        (None, true)
    );
    let required_timestamp = column(
        PhysicalType::Int64,
        LogicalType::Timestamp {
            adjusted_to_utc: false,
            unit: TimeUnit::Micros,
        },
        -1,
        false,
    );
    assert!(parse_raw_column_value(b"0000-00-00 00:00:00", &required_timestamp).is_err());

    let int64_decimal = column(
        PhysicalType::Int64,
        LogicalType::Decimal {
            precision: 19,
            scale: 0,
        },
        -1,
        false,
    );
    assert!(
        parse_raw_column_value(b"9223372036854775808", &int64_decimal)
            .unwrap_err()
            .to_string()
            .contains("does not fit in INT64")
    );

    let mut raw_bytes = b"abcd".to_vec();
    let parsed_bytes = parse_raw_column_value(
        &raw_bytes,
        &column(PhysicalType::ByteArray, LogicalType::None, -1, false),
    )
    .unwrap();
    raw_bytes[0] = b'z';
    assert_eq!(
        parsed_bytes,
        (Some(ColumnValue::Bytes(b"abcd".to_vec())), false)
    );

    let fixed_decimal = column(
        PhysicalType::FixedLenByteArray,
        LogicalType::Decimal {
            precision: 10,
            scale: 2,
        },
        4,
        false,
    );
    // 定长 DECIMAL 先定标为整数，再使用大端二补码填满声明宽度。
    assert_eq!(
        parse_raw_column_value(b"-1.23", &fixed_decimal).unwrap().0,
        Some(ColumnValue::FixedBytes(vec![0xff, 0xff, 0xff, 0x85]))
    );

    let fixed_bytes = column(PhysicalType::FixedLenByteArray, LogicalType::None, 4, false);
    let mut raw_fixed_bytes = b"wxyz".to_vec();
    let parsed_fixed_bytes = parse_raw_column_value(&raw_fixed_bytes, &fixed_bytes).unwrap();
    raw_fixed_bytes[0] = b'q';
    assert_eq!(
        parsed_fixed_bytes,
        (Some(ColumnValue::FixedBytes(b"wxyz".to_vec())), false)
    );
    assert!(
        parse_raw_column_value(b"abc", &fixed_bytes)
            .unwrap_err()
            .to_string()
            .contains("width mismatch")
    );

    let invalid_width = column(PhysicalType::FixedLenByteArray, LogicalType::None, 0, false);
    assert!(
        parse_raw_column_value(b"abcd", &invalid_width)
            .unwrap_err()
            .to_string()
            .contains("invalid fixed-size byte width")
    );
    let int96 = column(PhysicalType::Int96, LogicalType::None, -1, false);
    assert!(parse_raw_column_value(b"x", &int96).is_err());
}

#[test]
fn parse_raw_column_value_numeric_primitive_branches() {
    let cases = [
        (
            column(PhysicalType::Int32, LogicalType::None, -1, false),
            b"123".as_slice(),
            ColumnValue::Int32(123),
        ),
        (
            column(PhysicalType::Int64, LogicalType::None, -1, false),
            b"456".as_slice(),
            ColumnValue::Int64(456),
        ),
        (
            column(PhysicalType::Float, LogicalType::None, -1, false),
            b"1.5".as_slice(),
            ColumnValue::Float32(1.5),
        ),
        (
            column(PhysicalType::Double, LogicalType::None, -1, false),
            b"2.5".as_slice(),
            ColumnValue::Float64(2.5),
        ),
    ];
    for (column, raw, expected) in cases {
        assert_eq!(
            parse_raw_column_value(raw, &column).unwrap().0,
            Some(expected)
        );
    }
    assert!(
        parse_raw_column_value(
            b"bad",
            &column(PhysicalType::Float, LogicalType::None, -1, false)
        )
        .is_err()
    );
    assert!(
        parse_raw_column_value(
            b"bad",
            &column(PhysicalType::Double, LogicalType::None, -1, false)
        )
        .is_err()
    );
    let overflowing_fixed_decimal = column(
        PhysicalType::FixedLenByteArray,
        LogicalType::Decimal {
            precision: 3,
            scale: 2,
        },
        1,
        false,
    );
    assert!(
        parse_raw_column_value(b"1.28", &overflowing_fixed_decimal)
            .unwrap_err()
            .to_string()
            .contains("does not fit in 1 bytes")
    );
}

#[test]
fn append_column_value_appends_all_supported_physical_types() {
    let mut buffer = ColumnBuffer::default();
    // 每种 ColumnValue 必须落入与 Parquet 物理类型匹配的独立缓冲。
    let cases = [
        (PhysicalType::Boolean, ColumnValue::Bool(true)),
        (PhysicalType::Int32, ColumnValue::Int32(7)),
        (PhysicalType::Int64, ColumnValue::Int64(8)),
        (PhysicalType::Float, ColumnValue::Float32(1.25)),
        (PhysicalType::Double, ColumnValue::Float64(2.25)),
        (PhysicalType::ByteArray, ColumnValue::Bytes(b"a".to_vec())),
        (
            PhysicalType::FixedLenByteArray,
            ColumnValue::FixedBytes(b"bc".to_vec()),
        ),
    ];
    for (physical, value) in cases {
        append_column_value(
            &mut buffer,
            &column(physical, LogicalType::None, 2, false),
            value,
        )
        .unwrap();
    }
    assert_eq!(buffer.bool_values, vec![true]);
    assert_eq!(buffer.int32_values, vec![7]);
    assert_eq!(buffer.int64_values, vec![8]);
    assert_eq!(buffer.float32_values, vec![1.25]);
    assert_eq!(buffer.float64_values, vec![2.25]);
    assert_eq!(buffer.byte_array_values, vec![b"a".to_vec()]);
    assert_eq!(buffer.fixed_len_byte_array_values, vec![b"bc".to_vec()]);
}

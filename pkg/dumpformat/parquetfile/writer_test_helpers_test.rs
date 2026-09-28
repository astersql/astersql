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

// Parquet writer 列缓冲辅助逻辑的回归测试。
//
// 通过最小列描述验证物理类型到缓冲区的分派、可空列定义级别、缓冲重置复用，
// 以及不受支持的 Int96 类型拒绝路径。

use crate::column_buffer::new_column_buffer;
use crate::column_value::{append_column_value, write_column_batch};
use crate::{Column, ColumnInfo, ColumnType, ColumnValue, LogicalType, PhysicalType, TimeUnit};
use parquet::column::reader::ColumnReader;
use parquet::file::reader::RowGroupReader;

const _: fn(&dyn RowGroupReader, usize, usize, &[i32], &[i16], usize) = read_int32_column;
const _: fn(&dyn RowGroupReader, usize, usize, &[i64], &[i16], usize) = read_int64_column;
const _: fn(&dyn RowGroupReader, usize, usize, &[&str], &[i16], usize) = read_byte_array_column;
const _: fn(&dyn RowGroupReader, usize, usize, &[bool], &[i16], usize) = read_boolean_column;

/// 读取并核对一个 Int32 列，与 Go `readInt32Column` 的断言集合一致。
pub(super) fn read_int32_column(
    row_group: &dyn RowGroupReader,
    column: usize,
    rows: usize,
    expected: &[i32],
    expected_def: &[i16],
    expected_values: usize,
) {
    let ColumnReader::Int32ColumnReader(mut reader) = row_group.get_column_reader(column).unwrap()
    else {
        panic!("column {column} is not Int32");
    };
    let (mut values, mut def_levels) = (Vec::new(), Vec::new());
    let (total, values_read, levels_read) = reader
        .read_records(rows, Some(&mut def_levels), None, &mut values)
        .unwrap();
    // parquet-rs omits the physical level buffer for required columns, while
    // Arrow Go exposes one zero definition level per row.
    if def_levels.is_empty() {
        def_levels.resize(levels_read, 0);
    }
    assert_eq!(total, rows);
    assert_eq!(values_read, expected_values);
    assert_eq!(levels_read, rows);
    assert_eq!(values, expected);
    assert_eq!(def_levels, expected_def);
}

/// 读取并核对一个 Int64 列，与 Go `readInt64Column` 的断言集合一致。
pub(super) fn read_int64_column(
    row_group: &dyn RowGroupReader,
    column: usize,
    rows: usize,
    expected: &[i64],
    expected_def: &[i16],
    expected_values: usize,
) {
    let ColumnReader::Int64ColumnReader(mut reader) = row_group.get_column_reader(column).unwrap()
    else {
        panic!("column {column} is not Int64");
    };
    let (mut values, mut def_levels) = (Vec::new(), Vec::new());
    let (total, values_read, levels_read) = reader
        .read_records(rows, Some(&mut def_levels), None, &mut values)
        .unwrap();
    if def_levels.is_empty() {
        def_levels.resize(levels_read, 0);
    }
    assert_eq!(total, rows);
    assert_eq!(values_read, expected_values);
    assert_eq!(levels_read, rows);
    assert_eq!(values, expected);
    assert_eq!(def_levels, expected_def);
}

/// 读取并核对一个 ByteArray 列，与 Go `readByteArrayColumn` 的断言集合一致。
pub(super) fn read_byte_array_column(
    row_group: &dyn RowGroupReader,
    column: usize,
    rows: usize,
    expected: &[&str],
    expected_def: &[i16],
    expected_values: usize,
) {
    let ColumnReader::ByteArrayColumnReader(mut reader) =
        row_group.get_column_reader(column).unwrap()
    else {
        panic!("column {column} is not ByteArray");
    };
    let (mut values, mut def_levels) = (Vec::new(), Vec::new());
    let (total, values_read, levels_read) = reader
        .read_records(rows, Some(&mut def_levels), None, &mut values)
        .unwrap();
    if def_levels.is_empty() {
        def_levels.resize(levels_read, 0);
    }
    assert_eq!(total, rows);
    assert_eq!(values_read, expected_values);
    assert_eq!(levels_read, rows);
    assert_eq!(
        values.iter().map(|value| value.data()).collect::<Vec<_>>(),
        expected
            .iter()
            .map(|value| value.as_bytes())
            .collect::<Vec<_>>()
    );
    assert_eq!(def_levels, expected_def);
}

/// 读取并核对一个 Boolean 列，与 Go `readBooleanColumn` 的断言集合一致。
pub(super) fn read_boolean_column(
    row_group: &dyn RowGroupReader,
    column: usize,
    rows: usize,
    expected: &[bool],
    expected_def: &[i16],
    expected_values: usize,
) {
    let ColumnReader::BoolColumnReader(mut reader) = row_group.get_column_reader(column).unwrap()
    else {
        panic!("column {column} is not Boolean");
    };
    let (mut values, mut def_levels) = (Vec::new(), Vec::new());
    let (total, values_read, levels_read) = reader
        .read_records(rows, Some(&mut def_levels), None, &mut values)
        .unwrap();
    if def_levels.is_empty() {
        def_levels.resize(levels_read, 0);
    }
    assert_eq!(total, rows);
    assert_eq!(values_read, expected_values);
    assert_eq!(levels_read, rows);
    assert_eq!(values, expected);
    assert_eq!(def_levels, expected_def);
}

/// 构造测试所需的最小列描述；定长字节数组使用合法的两字节宽度。
fn column(physical: PhysicalType, nullable: bool) -> Column {
    Column {
        info: ColumnInfo {
            name: "v".into(),
            database_type_name: String::new(),
            nullable,
            precision: 0,
            scale: 0,
        },
        column_type: ColumnType {
            physical,
            logical: LogicalType::None,
            type_length: if physical == PhysicalType::FixedLenByteArray {
                2
            } else {
                -1
            },
            precision: 0,
            scale: 0,
        },
        allows_null_encoding: nullable,
        timestamp_unit: TimeUnit::Micros,
    }
}

#[test]
/// 验证可空列的定义级别独立于实际值保存，写出时仍按物理类型还原值。
fn typed_column_buffer_round_trips_values_and_definition_levels() {
    let column = column(PhysicalType::Int32, true);
    let mut buffer = new_column_buffer(&column, 2).unwrap();
    buffer.definition_levels.extend([1, 0]);
    append_column_value(&mut buffer, &column, ColumnValue::Int32(7)).unwrap();
    assert_eq!(buffer.definition_levels, vec![1, 0]);
    assert_eq!(
        write_column_batch(&buffer, &column).unwrap(),
        vec![ColumnValue::Int32(7)]
    );
}

#[test]
/// 覆盖浮点与字节缓冲的分派，并确认重置后无残留且 Int96 被拒绝。
fn column_buffers_round_trip_float_double_bytes_and_reset_capacity() {
    let cases = [
        (PhysicalType::Float, ColumnValue::Float32(1.5)),
        (PhysicalType::Double, ColumnValue::Float64(2.5)),
        (PhysicalType::ByteArray, ColumnValue::Bytes(b"a".to_vec())),
        (
            PhysicalType::FixedLenByteArray,
            ColumnValue::FixedBytes(b"bc".to_vec()),
        ),
    ];
    for (physical, value) in cases {
        let column = column(physical, false);
        let mut buffer = new_column_buffer(&column, 2).unwrap();
        append_column_value(&mut buffer, &column, value.clone()).unwrap();
        assert_eq!(write_column_batch(&buffer, &column).unwrap(), vec![value]);
        buffer.reset();
        assert!(write_column_batch(&buffer, &column).unwrap().is_empty());
    }
    assert!(new_column_buffer(&column(PhysicalType::Int96, false), 1).is_err());
}

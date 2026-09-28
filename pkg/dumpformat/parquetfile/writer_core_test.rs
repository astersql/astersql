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

// Parquet 写入器核心场景测试。
//
// 覆盖 SQL 列元数据到 Parquet schema 的映射、基础行写入与文件魔数，
// 并验证非法 DECIMAL 元数据及无符号 BIGINT 的定长十进制编码。

use crate::schema_builder::{Repetition, build_parquet_schema_from_columns};
use crate::writer::{Compression, ParquetWriter, WithCompression, WithDataPageSize};
use crate::{ColumnInfo, LogicalType, PhysicalType};
use parquet::basic::{ConvertedType, Type as ParquetPhysicalType};
use parquet::column::reader::ColumnReader;
use parquet::file::reader::{FileReader, SerializedFileReader};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
/// 可克隆的内存输出目标；写入器取得所有权后，测试仍可通过共享缓冲区检查结果。
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 简化测试中的 SQL 列元数据构造。
fn info(name: &str, kind: &str, nullable: bool, precision: i32, scale: i32) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        database_type_name: kind.into(),
        nullable,
        precision,
        scale,
    }
}

#[test]
/// 综合验证 schema 推导、配置传递、行写入与构造参数校验。
fn parquet_writer_schema_and_rows_cover_arrow_writer_scenarios() {
    let columns = [
        info("id", "INT", false, 0, 0),
        info("name", "VARCHAR", true, 0, 0),
        info("price", "DECIMAL", false, 10, 2),
        info("created_at", "DATETIME", false, 0, 0),
        info("flag", "BOOLEAN", true, 0, 0),
    ];
    let (schema, parsed) = build_parquet_schema_from_columns(&columns).unwrap();
    assert_eq!(schema.fields[0].physical, PhysicalType::Int32);
    assert_eq!(schema.fields[1].repetition, Repetition::Optional);
    assert_eq!(schema.fields[2].physical, PhysicalType::Int64);
    assert_eq!(
        schema.fields[2].logical,
        LogicalType::Decimal {
            precision: 10,
            scale: 2,
        }
    );
    assert_eq!(schema.fields[3].repetition, Repetition::Optional);
    // DATETIME 即使声明为非空，也要能把 MySQL 零日期编码成 NULL。
    assert!(parsed[3].allows_null_encoding);

    let sink = SharedSink::default();
    let output = sink.0.clone();
    let mut writer = ParquetWriter::new(
        sink,
        &columns,
        &[WithCompression(Compression::Zstd), WithDataPageSize(1024)],
    )
    .unwrap();
    writer
        .write(&[
            Some(b"1".to_vec()),
            Some(b"Alice".to_vec()),
            Some(b"123.45".to_vec()),
            Some(b"2024-01-02 03:04:05".to_vec()),
            Some(b"true".to_vec()),
        ])
        .unwrap();
    writer
        .write(&[
            Some(b"2".to_vec()),
            None,
            Some(b"-0.50".to_vec()),
            Some(b"0000-00-00 00:00:00".to_vec()),
            Some(b"false".to_vec()),
        ])
        .unwrap();
    writer.close().unwrap();
    assert!(
        writer
            .write(&[
                Some(b"3".to_vec()),
                None,
                Some(b"1.00".to_vec()),
                Some(b"2024-01-02 03:04:05".to_vec()),
                Some(b"true".to_vec()),
            ])
            .unwrap_err()
            .to_string()
            .contains("parquet writer is closed")
    );
    let output = output.lock().unwrap();
    assert!(output.starts_with(b"PAR1"));
    assert!(output.ends_with(b"PAR1"));

    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(&output)).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 2);
    assert_eq!(reader.num_row_groups(), 1);
    let descriptor = reader.metadata().file_metadata().schema_descr();
    let expected_schema = [
        (ParquetPhysicalType::INT32, ConvertedType::NONE, 0),
        (ParquetPhysicalType::BYTE_ARRAY, ConvertedType::UTF8, 1),
        (ParquetPhysicalType::INT64, ConvertedType::DECIMAL, 0),
        // parquet-rs 会从现代 TIMESTAMP logical type 派生 legacy converted type；
        // Arrow Go 的 metadata API 对同一 logical type 返回 NONE。
        (
            ParquetPhysicalType::INT64,
            ConvertedType::TIMESTAMP_MICROS,
            1,
        ),
        (ParquetPhysicalType::BYTE_ARRAY, ConvertedType::NONE, 1),
    ];
    for (column, expected) in descriptor.columns().iter().zip(expected_schema) {
        assert_eq!(column.physical_type(), expected.0);
        assert_eq!(column.converted_type(), expected.1);
        assert_eq!(column.max_def_level(), expected.2);
    }

    let row_group = reader.get_row_group(0).unwrap();
    let mut read_column = |index| row_group.get_column_reader(index).unwrap();
    match read_column(0) {
        ColumnReader::Int32ColumnReader(mut column) => {
            let mut values = Vec::new();
            assert_eq!(
                column.read_records(2, None, None, &mut values).unwrap(),
                (2, 2, 2)
            );
            assert_eq!(values, [1, 2]);
        }
        _ => panic!("id column has unexpected physical reader"),
    }
    match read_column(1) {
        ColumnReader::ByteArrayColumnReader(mut column) => {
            let (mut values, mut levels) = (Vec::new(), Vec::new());
            assert_eq!(
                column
                    .read_records(2, Some(&mut levels), None, &mut values)
                    .unwrap(),
                (2, 1, 2)
            );
            assert_eq!(levels, [1, 0]);
            assert_eq!(values[0].data(), b"Alice");
        }
        _ => panic!("name column has unexpected physical reader"),
    }
    match read_column(2) {
        ColumnReader::Int64ColumnReader(mut column) => {
            let mut values = Vec::new();
            assert_eq!(
                column.read_records(2, None, None, &mut values).unwrap(),
                (2, 2, 2)
            );
            assert_eq!(values, [12345, -50]);
        }
        _ => panic!("price column has unexpected physical reader"),
    }
    match read_column(3) {
        ColumnReader::Int64ColumnReader(mut column) => {
            let (mut values, mut levels) = (Vec::new(), Vec::new());
            assert_eq!(
                column
                    .read_records(2, Some(&mut levels), None, &mut values)
                    .unwrap(),
                (2, 1, 2)
            );
            assert_eq!(levels, [1, 0]);
            assert_eq!(values, [1_704_164_645_000_000]);
        }
        _ => panic!("created_at column has unexpected physical reader"),
    }
    match read_column(4) {
        ColumnReader::ByteArrayColumnReader(mut column) => {
            let (mut values, mut levels) = (Vec::new(), Vec::new());
            assert_eq!(
                column
                    .read_records(2, Some(&mut levels), None, &mut values)
                    .unwrap(),
                (2, 2, 2)
            );
            assert_eq!(levels, [1, 1]);
            assert_eq!(values[0].data(), b"true");
            assert_eq!(values[1].data(), b"false");
        }
        _ => panic!("flag column has unexpected physical reader"),
    }

    // 在进入底层 Parquet 类型构造前拒绝空列名与越界 DECIMAL scale。
    assert!(build_parquet_schema_from_columns(&[info("", "INT", false, 0, 0)]).is_err());
    assert!(build_parquet_schema_from_columns(&[info("bad", "DECIMAL", false, 10, 11)]).is_err());
    assert!(build_parquet_schema_from_columns(&[info("bad", "DECIMAL", false, 10, -1)]).is_err());
}

#[test]
/// 验证无符号 BIGINT 最大值不会溢出有符号 64 位物理类型。
fn unsigned_bigint_maps_to_nine_byte_fixed_decimal() {
    let columns = [info("u", "UNSIGNED BIGINT", false, 0, 0)];
    let (schema, _) = build_parquet_schema_from_columns(&columns).unwrap();
    assert_eq!(schema.fields[0].physical, PhysicalType::FixedLenByteArray);
    assert_eq!(schema.fields[0].type_length, 9);

    let sink = SharedSink::default();
    let output = sink.0.clone();
    let mut writer = ParquetWriter::new(
        sink,
        &columns,
        &[WithCompression(Compression::Uncompressed)],
    )
    .unwrap();
    writer
        .write(&[Some(b"18446744073709551615".to_vec())])
        .unwrap();
    writer.close().unwrap();
    let output = output.lock().unwrap();
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(&output)).unwrap();
    let descriptor = reader.metadata().file_metadata().schema_descr().column(0);
    assert_eq!(
        descriptor.physical_type(),
        ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY
    );
    assert_eq!(descriptor.converted_type(), ConvertedType::DECIMAL);
    assert_eq!(descriptor.type_length(), 9);
    let row_group = reader.get_row_group(0).unwrap();
    match row_group.get_column_reader(0).unwrap() {
        ColumnReader::FixedLenByteArrayColumnReader(mut column) => {
            let mut values = Vec::new();
            assert_eq!(
                column.read_records(1, None, None, &mut values).unwrap(),
                (1, 1, 1)
            );
            // 大端补码前置 0x00 以保持正号，因此 2^64-1 需要 9 字节。
            assert_eq!(
                values[0].data(),
                [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
            );
        }
        _ => panic!("unsigned BIGINT column has unexpected physical reader"),
    }
}

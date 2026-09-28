// testutils 迁移单元测试：校验 definition level 切片、多物理类型写出，
// 以及非法选项/生成器长度在落盘前被拒绝。
//
// Definition level >0 表示该行该列有实际值，切片时值缓冲会被压缩对齐。

// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use std::fs::File;

use crate::{
    ParquetColumn, ParquetValueBuffer, ParquetWriterOption, WriterProperty, calc_value_range,
    slice_column_data, write_parquet_file,
};
use parquet::basic::{ConvertedType, Type as PhysicalType};
use parquet::data_type::ByteArray;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::RowAccessor;

#[test]
/// 无/有 definition level 时，calc_value_range 与 slice_column_data 对齐压缩值区间。
fn value_ranges_and_slices_follow_definition_levels() {
    assert_eq!(calc_value_range(None, 2, 5).unwrap(), (2, 5));
    let levels = [0, 1, 0, 1, 1, 0];
    assert_eq!(calc_value_range(Some(&levels), 2, 6).unwrap(), (1, 3));

    let col = crate::ParquetColumnData::new(
        ParquetValueBuffer::Int64(vec![10, 30, 40]),
        Some(levels.to_vec()),
    );
    let (values, row_levels) = slice_column_data(&col, 2, 6).unwrap();
    assert_eq!(values, ParquetValueBuffer::Int64(vec![30, 40]));
    assert_eq!(row_levels, Some(vec![0, 1, 1, 0]));
}

#[test]
/// 各物理类型缓冲切片后 kind 不变，且 levels 随之截取。
fn every_go_value_buffer_variant_slices_without_changing_type() {
    let levels = Some(vec![1, 0, 1]);
    let cases = vec![
        ParquetValueBuffer::Int96(vec![Default::default(), Default::default()]),
        ParquetValueBuffer::Int64(vec![1, 2]),
        ParquetValueBuffer::Float64(vec![1.0, 2.0]),
        ParquetValueBuffer::ByteArray(vec![ByteArray::from("a"), ByteArray::from("b")]),
        ParquetValueBuffer::FixedLenByteArray(vec![vec![1].into(), vec![2].into()]),
        ParquetValueBuffer::Int32(vec![1, 2]),
        ParquetValueBuffer::Boolean(vec![true, false]),
    ];

    for values in cases {
        let original_kind = values.kind();
        let col = crate::ParquetColumnData::new(values, levels.clone());
        let (sliced, sliced_levels) = slice_column_data(&col, 1, 3).unwrap();
        assert_eq!(sliced.kind(), original_kind);
        assert_eq!(sliced.len(), 1);
        assert_eq!(sliced_levels, Some(vec![0, 1]));
    }
}

#[test]
/// 写出真实 Snappy Parquet，校验魔数、row group 数、压缩与可空列。
fn writes_a_real_snappy_parquet_file_with_requested_row_groups() {
    let dir = tempfile::tempdir().unwrap();
    let columns = vec![
        ParquetColumn::new(
            "id",
            PhysicalType::INT64,
            ConvertedType::INT_64,
            None,
            -1,
            -1,
            -1,
            |rows| {
                (
                    ParquetValueBuffer::Int64((0..rows).map(i64::from).collect()),
                    Some(vec![1; rows as usize]),
                )
            },
        ),
        ParquetColumn::new(
            "name",
            PhysicalType::BYTE_ARRAY,
            ConvertedType::UTF8,
            None,
            -1,
            -1,
            -1,
            |rows| {
                let levels = (0..rows)
                    .map(|row| if row % 2 == 0 { 1 } else { 0 })
                    .collect();
                (
                    ParquetValueBuffer::ByteArray(
                        (0..rows)
                            .filter(|row| row % 2 == 0)
                            .map(|i| ByteArray::from(format!("row-{i}").into_bytes()))
                            .collect(),
                    ),
                    Some(levels),
                )
            },
        ),
    ];

    write_parquet_file(
        dir.path().to_str().unwrap(),
        "rows.parquet",
        &columns,
        5,
        vec![
            ParquetWriterOption::WriterProperty(WriterProperty::MaxRowGroupLength(2)),
            ParquetWriterOption::WriterProperty(WriterProperty::CompressionFor(
                "name".into(),
                parquet::basic::Compression::UNCOMPRESSED,
            )),
        ],
    )
    .unwrap();

    let output = dir.path().join("rows.parquet");
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(&bytes[..4], b"PAR1");
    assert_eq!(&bytes[bytes.len() - 4..], b"PAR1");

    let reader = SerializedFileReader::new(File::open(output).unwrap()).unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 5);
    assert_eq!(reader.num_row_groups(), 3);
    assert_eq!(
        reader
            .metadata()
            .file_metadata()
            .schema_descr()
            .num_columns(),
        2
    );
    assert_eq!(
        reader.metadata().row_group(0).column(0).compression(),
        parquet::basic::Compression::SNAPPY
    );
    assert_eq!(
        reader.metadata().row_group(0).column(1).compression(),
        parquet::basic::Compression::UNCOMPRESSED
    );

    for (index, row) in reader.get_row_iter(None).unwrap().enumerate() {
        let row = row.unwrap();
        assert_eq!(row.get_long(0).unwrap(), index as i64);
        if index % 2 == 0 {
            assert_eq!(row.get_string(1).unwrap(), &format!("row-{index}"));
        } else {
            assert!(row.get_string(1).is_err());
        }
    }
}

#[test]
/// 覆盖 Go type switch 支持的全部物理类型并校验 schema。
fn writes_every_column_type_supported_by_the_go_type_switch() {
    let dir = tempfile::tempdir().unwrap();
    let columns = vec![
        ParquetColumn::new(
            "i96",
            PhysicalType::INT96,
            ConvertedType::NONE,
            None,
            -1,
            -1,
            -1,
            |_| {
                (
                    ParquetValueBuffer::Int96(vec![Default::default()]),
                    Some(vec![1]),
                )
            },
        ),
        ParquetColumn::new(
            "i64",
            PhysicalType::INT64,
            ConvertedType::NONE,
            None,
            -1,
            -1,
            -1,
            |_| (ParquetValueBuffer::Int64(vec![1]), Some(vec![1])),
        ),
        ParquetColumn::new(
            "f64",
            PhysicalType::DOUBLE,
            ConvertedType::NONE,
            None,
            -1,
            -1,
            -1,
            |_| (ParquetValueBuffer::Float64(vec![1.5]), Some(vec![1])),
        ),
        ParquetColumn::new(
            "bytes",
            PhysicalType::BYTE_ARRAY,
            ConvertedType::NONE,
            None,
            -1,
            -1,
            -1,
            |_| {
                (
                    ParquetValueBuffer::ByteArray(vec![ByteArray::from("x")]),
                    Some(vec![1]),
                )
            },
        ),
        ParquetColumn::new(
            "fixed",
            PhysicalType::FIXED_LEN_BYTE_ARRAY,
            ConvertedType::NONE,
            None,
            1,
            -1,
            -1,
            |_| {
                (
                    ParquetValueBuffer::FixedLenByteArray(vec![vec![7].into()]),
                    Some(vec![1]),
                )
            },
        ),
        ParquetColumn::new(
            "i32",
            PhysicalType::INT32,
            ConvertedType::NONE,
            None,
            -1,
            -1,
            -1,
            |_| (ParquetValueBuffer::Int32(vec![2]), Some(vec![1])),
        ),
        ParquetColumn::new(
            "bool",
            PhysicalType::BOOLEAN,
            ConvertedType::NONE,
            None,
            -1,
            -1,
            -1,
            |_| (ParquetValueBuffer::Boolean(vec![true]), Some(vec![1])),
        ),
    ];

    write_parquet_file(
        dir.path().to_str().unwrap(),
        "types.parquet",
        &columns,
        1,
        vec![],
    )
    .unwrap();

    let reader =
        SerializedFileReader::new(File::open(dir.path().join("types.parquet")).unwrap()).unwrap();
    let actual: Vec<_> = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|column| column.physical_type())
        .collect();
    assert_eq!(
        actual,
        vec![
            PhysicalType::INT96,
            PhysicalType::INT64,
            PhysicalType::DOUBLE,
            PhysicalType::BYTE_ARRAY,
            PhysicalType::FIXED_LEN_BYTE_ARRAY,
            PhysicalType::INT32,
            PhysicalType::BOOLEAN,
        ]
    );
}

#[test]
/// Go 对负 rows 仍调用生成器，row-group 循环为空并写出 0 行文件。
fn negative_rows_write_an_empty_file_like_go() {
    let dir = tempfile::tempdir().unwrap();
    let columns = vec![ParquetColumn::new(
        "id",
        PhysicalType::INT64,
        ConvertedType::INT_64,
        None,
        -1,
        -1,
        -1,
        |rows| {
            assert_eq!(rows, -1);
            (ParquetValueBuffer::Int64(Vec::new()), None)
        },
    )];

    write_parquet_file(
        dir.path().to_str().unwrap(),
        "negative.parquet",
        &columns,
        -1,
        vec![],
    )
    .unwrap();

    let reader =
        SerializedFileReader::new(File::open(dir.path().join("negative.parquet")).unwrap())
            .unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), 0);
    assert_eq!(reader.num_row_groups(), 0);
}

#[test]
/// 非法 range/选项/生成器长度应在落盘前失败且不残留文件。
fn rejects_bad_ranges_options_and_generator_lengths_before_leaving_a_file() {
    assert!(calc_value_range(Some(&[1]), 1, 0).is_err());
    assert!(calc_value_range(Some(&[1]), 0, 2).is_err());

    let dir = tempfile::tempdir().unwrap();
    let columns = vec![ParquetColumn::new(
        "id",
        PhysicalType::INT64,
        ConvertedType::INT_64,
        None,
        -1,
        -1,
        -1,
        |_| (ParquetValueBuffer::Int64(vec![]), Some(vec![])),
    )];

    let err = write_parquet_file(
        dir.path().to_str().unwrap(),
        "bad.parquet",
        &columns,
        1,
        vec![ParquetWriterOption::Unsupported("struct {}".into())],
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported parquet writer option type")
    );
    assert!(!dir.path().join("bad.parquet").exists());

    let err = write_parquet_file(
        dir.path().to_str().unwrap(),
        "short.parquet",
        &columns,
        1,
        vec![],
    )
    .unwrap_err();
    assert!(err.to_string().contains("definition levels"));
}

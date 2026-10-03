// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Parquet 解析器及其配套读取、类型转换与兼容逻辑的回归测试。
//
// 覆盖行与 row group 游标、定位约束、文件范围计算、Spark 旧历法转换，
// 以及分配器和读取器的内存记账；关键边界与 Go 版本的规则保持一致。

use crate::column_value::{ColumnValue, account_column_value_memory_bytes};
use crate::parser::{
    ColumnDescriptor, ConvertedType, EstimateParquetReaderMemory, NewParser, ParquetFile,
    ReadRowCount, RowGroup, SampleStatisticsFromParquet, TrackingAllocator, estimate_row_size,
};
use crate::reader_wrapper::{
    ColumnChunkMeta, FileMeta, InMemoryReaderBase, ReaderWrapper, row_group_range_from_meta,
};
use crate::spark_rebase::{
    AppVersion, SparkFileMeta, SparkRebaseMicrosLookup, floor_div_i64, floor_mod_i64,
    rebase_julian_to_gregorian_days, spark_rebase_time_zone_id, spark_version_from_metadata,
};
use crate::type_converter::{
    ConvertedInfo, Datum, MICROS_PER_DAY, convert_bytes, convert_int32, convert_int64,
    int96_to_unix_micros, new_int96,
};
use crate::{LogicalType, PhysicalType, TimeUnit};
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn dictionary_encoded_decimal_values_remain_stable_across_batches() {
    use crate::file_parser::{FileParser, ImportParser};
    use crate::source_reader::{RangeOpener, SourceReader};
    use astersql_lightning_mydump::{Datum as ImportDatum, MydumpError, Parser as _};
    use parquet::data_type::{ByteArray, ByteArrayType, FixedLenByteArray, FixedLenByteArrayType};
    use parquet::file::{
        properties::{WriterProperties, WriterVersion},
        reader::FileReader,
        writer::SerializedFileWriter,
    };
    use std::io::Cursor;

    const ROWS: usize = 256;
    let schema = Arc::new(parquet::schema::parser::parse_message_type(
        "message schema { OPTIONAL BYTE_ARRAY positive (DECIMAL(9,2)); OPTIONAL BYTE_ARRAY negative (DECIMAL(9,2)); OPTIONAL FIXED_LEN_BYTE_ARRAY(4) fixed_len (DECIMAL(9,2)); OPTIONAL BYTE_ARRAY oversized (DECIMAL(75,2)); }",
    ).unwrap());
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        schema,
        Arc::new(
            WriterProperties::builder()
                .set_writer_version(WriterVersion::PARQUET_2_0)
                .set_dictionary_enabled(true)
                .build(),
        ),
    )
    .unwrap();
    let mut oversized = vec![0; 33];
    oversized[1] = 1;
    let inputs = [
        vec![0x30, 0x39],
        vec![0xcf, 0xc7],
        vec![0, 0, 0x30, 0x39],
        oversized,
    ];
    let expected = [
        "123.45",
        "-123.45",
        "123.45",
        "4523128485832663883733241601901871400518358776001584532791311875309106626.56",
    ];
    let info = ConvertedInfo {
        converted: ConvertedType::Decimal,
        scale: 2,
        adjusted_to_utc: false,
        timezone_offset_seconds: 0,
        spark_rebase: None,
    };
    let mut group = writer.next_row_group().unwrap();
    for (index, input) in inputs.iter().enumerate() {
        let original = input.clone();
        for _ in 0..ROWS {
            assert_eq!(
                convert_bytes(input, &info).unwrap(),
                Datum::Decimal(expected[index].into())
            );
            assert_eq!(input, &original, "conversion mutated dictionary bytes");
        }
        let mut column = group.next_column().unwrap().unwrap();
        if index == 2 {
            let values = vec![FixedLenByteArray::from(ByteArray::from(input.clone())); ROWS];
            column
                .typed::<FixedLenByteArrayType>()
                .write_batch(&values, Some(&[1; ROWS]), None)
                .unwrap();
        } else {
            let values = vec![ByteArray::from(input.clone()); ROWS];
            column
                .typed::<ByteArrayType>()
                .write_batch(&values, Some(&[1; ROWS]), None)
                .unwrap();
        }
        column.close().unwrap();
    }
    group.close().unwrap();
    let bytes = writer.into_inner().unwrap();
    let reader = crate::parser::open_file_reader(bytes::Bytes::copy_from_slice(&bytes)).unwrap();
    for column in reader.metadata().row_group(0).columns() {
        assert!(
            column.dictionary_page_offset().is_some(),
            "{} is not dictionary encoded",
            column.column_path()
        );
    }
    let data = Arc::new(bytes);
    let size = data.len() as u64;
    let open: RangeOpener = Arc::new(move |start, end| {
        Ok(Box::new(Cursor::new(
            data[start as usize..end as usize].to_vec(),
        )))
    });
    let source = SourceReader::prepare(size as i64, || Ok(size), open).unwrap();
    let mut decoder = FileParser::new(source).unwrap();
    // Reuse the dictionary across many decoder batches as well as parser rows.
    decoder.set_batch_size(7).unwrap();
    let mut parser = ImportParser::new(decoder);
    for row in 0..ROWS {
        parser.ReadRow().unwrap();
        let actual = parser.LastRow();
        assert_eq!(actual.row.len(), expected.len());
        for (value, text) in actual.row.iter().zip(expected) {
            assert_eq!(
                value,
                &ImportDatum::Bytes(text.as_bytes().to_vec()),
                "row {row}"
            );
        }
        parser.RecycleRow(actual);
    }
    assert!(matches!(parser.ReadRow(), Err(MydumpError::Eof)));
}

// 构造最小列描述；各用例只覆盖显式指定的物理类型与逻辑类型。
fn descriptor(name: &str, physical: PhysicalType, logical: LogicalType) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.into(),
        physical,
        logical,
        converted: ConvertedType::None,
        adjusted_to_utc: false,
    }
}

// 固定两列 schema，让解析器用例只需关注 row group 与游标行为。
fn two_column_file(groups: Vec<RowGroup>) -> ParquetFile {
    ParquetFile {
        columns: vec![
            descriptor("ID", PhysicalType::Int32, LogicalType::None),
            descriptor("Name", PhysicalType::ByteArray, LogicalType::String),
        ],
        row_groups: groups,
        source_size: 128,
        ..ParquetFile::default()
    }
}

#[test]
fn parquet_parser_reads_rows_columns_and_positions() {
    let file = two_column_file(vec![RowGroup {
        rows: vec![
            vec![
                Some(ColumnValue::Int32(1)),
                Some(ColumnValue::Bytes(b"alice".to_vec())),
            ],
            vec![Some(ColumnValue::Int32(2)), None],
        ],
        compressed_bytes: 32,
    }]);
    let mut parser = NewParser(file).unwrap();
    // 列名统一小写；Pos 同时保留物理读取进度和可独立设置的逻辑 row_id。
    assert_eq!(parser.columns(), &["id", "name"]);
    parser.read_row().unwrap();
    assert_eq!(parser.last_row().row_id, 1);
    assert_eq!(parser.pos(), (1, 1));
    assert_eq!(parser.scanned_pos(), 64);
    parser.read_row().unwrap();
    assert_eq!(parser.last_row().row_id, 2);
    assert!(parser.read_row().unwrap_err().to_string().contains("EOF"));
}

#[test]
fn parquet_parser_moves_across_multiple_row_groups() {
    let file = two_column_file(vec![
        RowGroup {
            rows: vec![vec![Some(ColumnValue::Int32(1)), None]],
            compressed_bytes: 10,
        },
        // Go 每次只推进一个 row group；进入空组的这次读取会返回 EOF。
        RowGroup::default(),
        RowGroup {
            rows: vec![vec![
                Some(ColumnValue::Int32(2)),
                Some(ColumnValue::Bytes(b"b".to_vec())),
            ]],
            compressed_bytes: 10,
        },
    ]);
    let mut parser = NewParser(file).unwrap();
    parser.read_row().unwrap();
    assert!(parser.read_row().unwrap_err().to_string().contains("EOF"));
    parser.read_row().unwrap();
    assert_eq!(parser.last_row().row[0], Some(ColumnValue::Int32(2)));
    assert_eq!(parser.last_row().row_id, 3);
}

#[test]
fn parquet_parser_set_pos_skips_forward_and_allows_go_style_row_id_reset() {
    let file = two_column_file(vec![RowGroup {
        rows: (0..4)
            .map(|id| vec![Some(ColumnValue::Int32(id)), None])
            .collect(),
        compressed_bytes: 16,
    }]);
    let mut parser = NewParser(file).unwrap();
    // set_pos 先推进物理行游标，再独立设置逻辑 row_id；Go 对负 toRead 不执行循环。
    parser.set_pos(2, 20).unwrap();
    assert_eq!(parser.pos(), (2, 20));
    parser.read_row().unwrap();
    assert_eq!(parser.last_row().row[0], Some(ColumnValue::Int32(2)));
    parser.set_pos(1, 1).unwrap();
    assert_eq!(parser.pos(), (3, 1));
    parser.read_row().unwrap();
    assert_eq!(parser.last_row().row[0], Some(ColumnValue::Int32(3)));
}

#[test]
fn parquet_parser_rejects_unsupported_containers_and_accepts_nanos() {
    let mut unsupported = ParquetFile {
        columns: vec![descriptor(
            "items",
            PhysicalType::ByteArray,
            LogicalType::None,
        )],
        ..ParquetFile::default()
    };
    unsupported.columns[0].converted = ConvertedType::List;
    assert!(NewParser(unsupported).is_err());

    let nanos = ParquetFile {
        columns: vec![descriptor(
            "ts",
            PhysicalType::Int64,
            LogicalType::Timestamp {
                adjusted_to_utc: true,
                unit: TimeUnit::Nanos,
            },
        )],
        ..ParquetFile::default()
    };
    assert!(NewParser(nanos).is_ok());
}

#[test]
fn row_size_row_count_and_sampling_match_go_rules() {
    let row = vec![
        Some(ColumnValue::Bytes(b"abc".to_vec())),
        Some(ColumnValue::Int64(7)),
        None,
    ];
    // 字节列按实际长度计数，其他非空值按 8 字节估算，NULL 不占行数据字节。
    assert_eq!(estimate_row_size(&row), 11);
    let file = ParquetFile {
        columns: vec![
            descriptor("s", PhysicalType::ByteArray, LogicalType::String),
            descriptor("i", PhysicalType::Int64, LogicalType::None),
            descriptor("n", PhysicalType::ByteArray, LogicalType::None),
        ],
        row_groups: vec![RowGroup {
            rows: vec![row.clone(), row],
            compressed_bytes: 20,
        }],
        ..ParquetFile::default()
    };
    assert_eq!(ReadRowCount(&file), 2);
    assert_eq!(SampleStatisticsFromParquet(file).unwrap(), (2, 11.0));

    let first_group_empty = two_column_file(vec![
        RowGroup::default(),
        RowGroup {
            rows: vec![vec![Some(ColumnValue::Int32(1)), None]],
            compressed_bytes: 8,
        },
    ]);
    assert_eq!(
        SampleStatisticsFromParquet(first_group_empty).unwrap(),
        (0, 0.0)
    );
}

#[test]
fn reader_wrapper_supports_forward_gaps_random_reads_and_close() {
    let data = Arc::new((0_u8..100).collect::<Vec<_>>());
    let mut reader = ReaderWrapper::new(data, 0).unwrap();
    let mut out = [0; 4];
    // 小跨度前跳可消费 skip buffer，回读则必须重新定位；关闭后禁止继续读取。
    assert_eq!(reader.read_at(&mut out, 10).unwrap(), 4);
    assert_eq!(out, [10, 11, 12, 13]);
    assert_eq!(reader.read_at(&mut out, 2).unwrap(), 4);
    assert_eq!(out, [2, 3, 4, 5]);
    reader.close();
    assert!(reader.read_at(&mut out, 0).is_err());
}

#[test]
fn in_memory_reader_and_row_group_ranges_cover_boundaries() {
    let meta = FileMeta {
        source_size: 200,
        old_parquet_mr: false,
        row_groups: vec![vec![
            ColumnChunkMeta {
                data_page_offset: 40,
                dictionary_page_offset: Some(20),
                total_compressed_size: 10,
            },
            ColumnChunkMeta {
                data_page_offset: 100,
                dictionary_page_offset: None,
                total_compressed_size: 20,
            },
        ]],
    };
    // row group 从最早的字典页起算，并覆盖所有列块的压缩范围。
    let range = row_group_range_from_meta(&meta, 0).unwrap();
    assert_eq!((range.start, range.end), (20, 120));
    assert_eq!(range.column_starts, vec![20, 100]);
    let file = (0_u8..=199).collect::<Vec<_>>();
    let base = InMemoryReaderBase::new(&file, range).unwrap();
    let mut out = [0; 3];
    assert_eq!(base.read_at(&mut out, 100).unwrap(), 3);
    assert_eq!(out, [100, 101, 102]);
    assert!(base.read_at(&mut out, 19).is_err());
}

#[test]
fn old_parquet_mr_range_adds_dictionary_header_padding() {
    let meta = FileMeta {
        source_size: 200,
        old_parquet_mr: true,
        row_groups: vec![vec![ColumnChunkMeta {
            data_page_offset: 20,
            dictionary_page_offset: None,
            total_compressed_size: 10,
        }]],
    };
    // 旧 parquet-mr 可能漏计字典页头，范围尾部需在文件边界内补最多 100 字节。
    let range = row_group_range_from_meta(&meta, 0).unwrap();
    assert_eq!((range.start, range.end), (20, 130));
}

#[test]
fn binary_decimal_and_integer_decimal_conversion_match_expected_strings() {
    let decimal = ConvertedInfo {
        converted: ConvertedType::Decimal,
        scale: 3,
        adjusted_to_utc: false,
        timezone_offset_seconds: 0,
        spark_rebase: None,
    };
    // 二进制定点数使用有符号补码，scale 决定小数点位置。
    assert_eq!(
        convert_bytes(&[0x01], &decimal).unwrap(),
        Datum::Decimal("0.001".into())
    );
    assert_eq!(
        convert_bytes(&[0xff], &decimal).unwrap(),
        Datum::Decimal("-0.001".into())
    );
    assert_eq!(
        convert_int64(123, &decimal).unwrap(),
        Datum::Decimal("0.123".into())
    );
    assert_eq!(
        convert_int64(
            -7,
            &ConvertedInfo {
                scale: 2,
                ..decimal
            }
        )
        .unwrap(),
        Datum::Decimal("-0.07".into())
    );
}

#[test]
fn date_conversion_only_rebases_with_spark_legacy_lookup() {
    let plain = ConvertedInfo {
        converted: ConvertedType::Date,
        scale: 0,
        adjusted_to_utc: false,
        timezone_offset_seconds: 0,
        spark_rebase: None,
    };
    // 普通 DATE 保留原始 epoch day；只有 Spark legacy 元数据才触发历法重置。
    assert_eq!(
        convert_int32(-200_000, &plain).unwrap(),
        Datum::TimeMicros(-200_000_i64 * MICROS_PER_DAY)
    );
    let legacy = ConvertedInfo {
        spark_rebase: Some(SparkRebaseMicrosLookup::new("UTC").unwrap()),
        ..plain
    };
    assert_eq!(
        convert_int32(-200_000, &legacy).unwrap(),
        Datum::TimeMicros(rebase_julian_to_gregorian_days(-200_000) as i64 * MICROS_PER_DAY)
    );
}

#[test]
fn adjusted_timestamp_conversion_applies_parser_location_offset() {
    let info = ConvertedInfo {
        converted: ConvertedType::TimestampMicros,
        scale: 0,
        adjusted_to_utc: true,
        timezone_offset_seconds: 8 * 3600,
        spark_rebase: None,
    };
    assert_eq!(
        convert_int64(1_000_000, &info).unwrap(),
        Datum::TimeMicros(28_801_000_000)
    );
}

#[test]
fn int96_round_trips_pre_and_post_epoch_micros() {
    // 负时间戳要求按向下取整拆分日期与日内时间，不能依赖向零截断。
    for micros in [-86_400_000_001_i64, -1, 0, 1, 86_400_000_001] {
        assert_eq!(int96_to_unix_micros(new_int96(micros)), micros);
    }
}

#[test]
fn spark_metadata_version_timezone_and_rebase_policy_match_go() {
    let mut meta = SparkFileMeta {
        created_by: "spark version 2.4.8".into(),
        key_values: BTreeMap::new(),
    };
    assert_eq!(spark_version_from_metadata(&meta).unwrap().major, 2);
    let cutoff = AppVersion::parse_spark("3.0.0").unwrap();
    assert_eq!(
        spark_rebase_time_zone_id(&meta, &cutoff, "legacy", "UTC"),
        "UTC"
    );
    meta.key_values.insert(
        "org.apache.spark.timeZone".into(),
        "America/Los_Angeles".into(),
    );
    assert_eq!(
        spark_rebase_time_zone_id(&meta, &cutoff, "legacy", "UTC"),
        "America/Los_Angeles"
    );
    let modern = SparkRebaseMicrosLookup::new("UTC").unwrap();
    assert_eq!(modern.rebase(0).unwrap(), 0);
    assert!(SparkRebaseMicrosLookup::new("not/a-zone").is_err());
}

#[test]
fn parser_calendar_math_uses_floor_division_for_negative_values() {
    assert_eq!(floor_div_i64(-1, 86_400), -1);
    assert_eq!(floor_mod_i64(-1, 86_400), 86_399);
    assert_eq!(floor_div_i64(86_401, 86_400), 1);
}

#[test]
fn tracking_allocator_reallocate_preserves_prefix_capacity_and_peak() {
    let mut allocator = TrackingAllocator::default();
    let (id, mut bytes) = allocator.allocate(8);
    bytes.copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let (same_id, shrunk) = allocator.reallocate(id, 4, &bytes);
    assert_eq!(same_id, id);
    assert_eq!(shrunk, vec![1, 2, 3, 4]);
    assert_eq!(allocator.current(), 8 + 64);
    // 扩容会先分配新块再释放旧块，因此历史峰值必须同时计入两块及对齐开销。
    let (new_id, grown) = allocator.reallocate(same_id, 16, &shrunk);
    assert_ne!(new_id, same_id);
    assert_eq!(&grown[..4], &[1, 2, 3, 4]);
    assert_eq!(allocator.current(), 16 + 64);
    assert_eq!(allocator.peak(), 8 + 16 + 2 * 64);
    allocator.free(new_id);
    assert_eq!(allocator.current(), 0);
}

#[test]
fn tracking_allocator_zero_length_reuses_backing_allocation() {
    let mut allocator = TrackingAllocator::default();
    let (id, bytes) = allocator.allocate(8);
    let (same_id, zero) = allocator.reallocate(id, 0, &bytes);
    assert_eq!(same_id, id);
    assert!(zero.is_empty());
    assert_eq!(allocator.current(), 8 + 64);
    let (grown_id, grown) = allocator.reallocate(same_id, 16, &zero);
    assert_eq!(grown.len(), 16);
    assert_eq!(allocator.peak(), 8 + 16 + 2 * 64);
    allocator.free(grown_id);
    assert_eq!(allocator.current(), 0);
}

#[test]
fn reader_memory_estimate_includes_preload_values_and_level_buffers() {
    let file = ParquetFile {
        columns: vec![descriptor("v", PhysicalType::ByteArray, LogicalType::None)],
        row_groups: vec![RowGroup {
            rows: vec![vec![Some(ColumnValue::Bytes(b"abc".to_vec()))]],
            compressed_bytes: 100,
        }],
        ..ParquetFile::default()
    };
    // 估算值由整组预读、列值持有内存和每列 definition/repetition level 缓冲组成。
    let expected = 100
        + account_column_value_memory_bytes(&ColumnValue::Bytes(b"abc".to_vec()))
        + (crate::parser::READ_BATCH_SIZE * 4) as i64;
    assert_eq!(EstimateParquetReaderMemory(&file).unwrap(), expected);
}

#[test]
fn parquet_scanned_pos_by_read_rows() {
    for (rows, size) in [(10, 101), (0, 101), (97, 10_i64 << 30)] {
        let mut file = two_column_file(vec![RowGroup {
            rows: (0..rows)
                .map(|i| vec![Some(ColumnValue::Int32(i)), None])
                .collect(),
            compressed_bytes: 0,
        }]);
        file.source_size = size;
        let mut parser = NewParser(file).unwrap();
        let mut previous = 0;
        for consumed in 0..=rows {
            let pos = parser.ScannedPos();
            let expected = if rows == 0 || consumed == rows {
                size
            } else {
                ((consumed as f64 / rows as f64) * size as f64) as i64
            };
            assert_eq!(pos, expected);
            assert!(pos >= previous && pos <= size);
            previous = pos;
            if consumed < rows {
                parser.ReadRow().unwrap();
            }
        }
    }
}

fn progress_parquet_source(
    rows: i64,
    strategy: usize,
) -> (crate::source_reader::SourceReader, i64) {
    use crate::source_reader::{RangeOpener, SourceReader};
    use parquet::data_type::Int64Type;
    use parquet::file::{properties::WriterProperties, writer::SerializedFileWriter};
    use std::io::Cursor;
    let schema = Arc::new(
        parquet::schema::parser::parse_message_type(
            "message schema { REQUIRED INT64 v; REQUIRED INT64 late_v; }",
        )
        .unwrap(),
    );
    let properties = Arc::new(WriterProperties::builder().build());
    let mut bytes = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut bytes, schema, properties).unwrap();
    for start in (0..rows).step_by(9) {
        let mut group = writer.next_row_group().unwrap();
        for factor in [1, 2] {
            let mut column = group.next_column().unwrap().unwrap();
            let values = (start..(start + 9).min(rows))
                .map(|i| i * factor)
                .collect::<Vec<_>>();
            column
                .typed::<Int64Type>()
                .write_batch(&values, None, None)
                .unwrap();
            column.close().unwrap();
        }
        group.close().unwrap();
    }
    writer.close().unwrap();
    let data = Arc::new(bytes);
    let size = data.len() as u64;
    let open: RangeOpener = Arc::new(move |start, end| {
        Ok(Box::new(Cursor::new(
            data[start as usize..end as usize].to_vec(),
        )))
    });
    let (whole, group) = match strategy {
        0 => (0, 128 << 20),
        1 => (size, 128 << 20),
        _ => (0, 1),
    };
    let source =
        SourceReader::prepare_with_thresholds(size as i64, || Ok(size), open, whole, group)
            .unwrap();
    assert_eq!(source.whole_file_preloaded(), strategy == 1);
    (source, size as i64)
}

#[test]
fn parquet_import_scanned_pos_tracks_rows_across_reader_strategies() {
    use crate::file_parser::{FileParser, ImportParser};
    use astersql_lightning_mydump::{Datum as D, MydumpError, Parser as _};
    for strategy in 0..3 {
        let (source, size) = progress_parquet_source(50, strategy);
        let mut parser = ImportParser::new(FileParser::new(source).unwrap());
        assert_eq!(parser.inner.total_rows(), 50);
        assert_eq!(parser.ScannedPos().unwrap(), 0);
        let mut previous = 0;
        for i in 0..50 {
            parser.ReadRow().unwrap();
            let row = parser.LastRow();
            assert_eq!(row.row, vec![D::I64(i), D::I64(i * 2)]);
            assert_eq!(row.length, 16);
            let pos = parser.ScannedPos().unwrap();
            assert_eq!(pos, size * (i + 1) / 50);
            assert!(pos >= previous && pos <= size);
            previous = pos;
            parser.RecycleRow(row);
        }
        assert_eq!(parser.ScannedPos().unwrap(), size);
        assert!(matches!(parser.ReadRow(), Err(MydumpError::Eof)));
        assert_eq!(parser.ScannedPos().unwrap(), size);
    }
}

#[test]
fn parquet_import_scanned_pos_set_pos_and_empty_file() {
    use crate::file_parser::{FileParser, ImportParser};
    use astersql_lightning_mydump::{MydumpError, Parser as _};
    let (source, size) = progress_parquet_source(10, 1);
    let mut parser = ImportParser::new(FileParser::new(source).unwrap());
    parser.SetPos(4, 40).unwrap();
    assert_eq!(parser.Pos(), (4, 40));
    assert_eq!(
        parser.ScannedPos().unwrap(),
        ((4.0 / 10.0) * size as f64) as i64
    );
    parser.SetPos(1, 4).unwrap();
    assert_eq!(parser.Pos(), (4, 4));
    assert!(matches!(parser.SetPos(11, 99), Err(MydumpError::Eof)));
    assert_eq!(parser.Pos(), (10, 4));
    assert_eq!(parser.ScannedPos().unwrap(), size);
    let (source, size) = progress_parquet_source(0, 0);
    let mut parser = ImportParser::new(FileParser::new(source).unwrap());
    assert_eq!(parser.ScannedPos().unwrap(), size);
    assert!(matches!(parser.ReadRow(), Err(MydumpError::Eof)));
    assert_eq!(parser.ScannedPos().unwrap(), size);
}

fn logical_parquet_source(bytes: Vec<u8>) -> crate::source_reader::SourceReader {
    use crate::source_reader::{RangeOpener, SourceReader};
    let data = Arc::new(bytes);
    let size = data.len() as u64;
    let open: RangeOpener = Arc::new(move |start, end| {
        Ok(Box::new(std::io::Cursor::new(
            data[start as usize..end as usize].to_vec(),
        )))
    });
    SourceReader::prepare(size as i64, || Ok(size), open).unwrap()
}

fn logical_int64_file(annotation: &str, values: &[i64], metadata: Option<&str>) -> Vec<u8> {
    use parquet::data_type::Int64Type;
    use parquet::file::{properties::WriterProperties, writer::SerializedFileWriter};
    let schema = Arc::new(
        parquet::schema::parser::parse_message_type(&format!(
            "message schema {{ OPTIONAL INT64 value ({annotation}); }}"
        ))
        .unwrap(),
    );
    let mut props = WriterProperties::builder().set_created_by("parquet-mr version 1.10.1".into());
    if let Some(zone) = metadata {
        props = props.set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new(
                "org.apache.spark.legacyDateTime".into(),
                Some("".into()),
            ),
            parquet::file::metadata::KeyValue::new(
                "org.apache.spark.timeZone".into(),
                Some(zone.into()),
            ),
        ]));
    }
    let mut writer =
        SerializedFileWriter::new(Vec::new(), schema, Arc::new(props.build())).unwrap();
    let mut group = writer.next_row_group().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    column
        .typed::<Int64Type>()
        .write_batch(values, Some(&vec![1; values.len()]), None)
        .unwrap();
    column.close().unwrap();
    group.close().unwrap();
    writer.into_inner().unwrap()
}

#[test]
fn logical_nanos_fixture_reaches_import_parser_with_midnight_carry() {
    use crate::file_parser::{FileParser, ImportParser};
    use astersql_lightning_mydump::{Datum as D, Parser as _};
    let bytes =
        include_bytes!("../../../tests/realtikvtest/importintotest/logical-time-nanos.parquet");
    let mut parser =
        ImportParser::new(FileParser::new(logical_parquet_source(bytes.to_vec())).unwrap());
    for expected in [
        ["1", "23:59:59.999999", "1969-12-31 23:59:59.999999"],
        ["2", "24:00:00.000000", "2020-10-29 09:27:52.356956"],
    ] {
        parser.ReadRow().unwrap();
        let actual = parser.LastRow();
        assert_eq!(actual.row[0], D::I64(expected[0].parse().unwrap()));
        for (value, text) in actual.row[1..].iter().zip(&expected[1..]) {
            assert_eq!(value, &D::Bytes(text.as_bytes().to_vec()));
        }
    }
    assert!(parser.ReadRow().is_err());
}

#[test]
fn logical_time_preserves_duration_and_wraps_only_utc_adjusted_wall_clock() {
    use crate::file_parser::{FileParser, ImportParser};
    use astersql_lightning_mydump::{Datum as D, Parser as _};
    for (annotation, values, expected) in [
        (
            "TIME(MICROS,false)",
            vec![123_456_789],
            vec!["00:02:03.456789"],
        ),
        (
            "TIME(MICROS,true)",
            vec![86_399_999_999],
            vec!["07:59:59.999999"],
        ),
        (
            "TIME(NANOS,false)",
            vec![86_400_000_000_000 - 501, 86_400_000_000_000 - 500],
            vec!["23:59:59.999999", "24:00:00.000000"],
        ),
    ] {
        let mut parser = ImportParser::new(
            FileParser::new_with_location(
                logical_parquet_source(logical_int64_file(annotation, &values, None)),
                "Asia/Shanghai",
            )
            .unwrap(),
        );
        for text in expected {
            parser.ReadRow().unwrap();
            assert_eq!(
                parser.LastRow().row,
                vec![D::Bytes(text.as_bytes().to_vec())]
            );
        }
    }
    for value in [-1, 86_400_000_000_000] {
        let mut parser = FileParser::new(logical_parquet_source(logical_int64_file(
            "TIME(NANOS,false)",
            &[value],
            None,
        )))
        .unwrap();
        assert!(
            parser
                .read_row()
                .unwrap_err()
                .0
                .contains("outside the valid range")
        );
    }
}

#[test]
fn logical_timestamp_nanos_skips_spark_rebase_and_preserves_adjustment() {
    use crate::file_parser::FileParser;
    for (adjusted, expected) in [
        (false, 1_603_963_672_356_956),
        (true, 1_603_992_472_356_956),
    ] {
        let mut parser = FileParser::new_with_location(
            logical_parquet_source(logical_int64_file(
                &format!("TIMESTAMP(NANOS,{adjusted})"),
                &[1_603_963_672_356_956_000],
                Some("Unknown/SparkZone"),
            )),
            "Asia/Shanghai",
        )
        .unwrap();
        assert_eq!(
            parser.read_row().unwrap(),
            vec![Datum::TimeMicros(expected)]
        );
    }
}

fn logical_null_file(physical: parquet::basic::Type, present: bool) -> Vec<u8> {
    logical_null_file_rows(physical, &[i16::from(present)])
}

fn logical_null_file_rows(physical: parquet::basic::Type, levels: &[i16]) -> Vec<u8> {
    use parquet::{
        basic::{LogicalType as L, Repetition},
        data_type::*,
        file::writer::SerializedFileWriter,
        schema::types::Type,
    };
    let field = Type::primitive_type_builder("value", physical)
        .with_repetition(Repetition::OPTIONAL)
        .with_logical_type(Some(L::Unknown))
        .with_length(if physical == parquet::basic::Type::FIXED_LEN_BYTE_ARRAY {
            2
        } else {
            -1
        })
        .build()
        .unwrap();
    let schema = Arc::new(
        Type::group_type_builder("schema")
            .with_fields(vec![Arc::new(field)])
            .build()
            .unwrap(),
    );
    let mut writer = SerializedFileWriter::new(Vec::new(), schema, Default::default()).unwrap();
    let mut group = writer.next_row_group().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    macro_rules! write {
        ($ty:ty, $value:expr) => {{
            let values = vec![$value; levels.iter().filter(|&&level| level > 0).count()];
            column
                .typed::<$ty>()
                .write_batch(&values, Some(levels), None)
                .unwrap();
        }};
    }
    use parquet::basic::Type as P;
    match physical {
        P::BOOLEAN => write!(BoolType, true),
        P::INT32 => write!(Int32Type, 1),
        P::INT64 => write!(Int64Type, 1),
        P::INT96 => write!(Int96Type, Int96::default()),
        P::FLOAT => write!(FloatType, 1.0),
        P::DOUBLE => write!(DoubleType, 1.0),
        P::BYTE_ARRAY => write!(ByteArrayType, ByteArray::from("ab")),
        P::FIXED_LEN_BYTE_ARRAY => {
            write!(FixedLenByteArrayType, FixedLenByteArray::from(vec![1, 2]))
        }
    }
    column.close().unwrap();
    group.close().unwrap();
    writer.into_inner().unwrap()
}

#[test]
fn logical_null_accepts_nulls_and_rejects_non_null_values_for_every_physical_type() {
    use crate::file_parser::FileParser;
    use parquet::basic::Type as P;
    for physical in [
        P::BOOLEAN,
        P::INT32,
        P::INT64,
        P::INT96,
        P::FLOAT,
        P::DOUBLE,
        P::BYTE_ARRAY,
        P::FIXED_LEN_BYTE_ARRAY,
    ] {
        let mut parser =
            FileParser::new(logical_parquet_source(logical_null_file(physical, false))).unwrap();
        assert_eq!(parser.read_row().unwrap(), vec![Datum::Null]);
        let mut parser =
            FileParser::new(logical_parquet_source(logical_null_file(physical, true))).unwrap();
        assert!(
            parser
                .read_row()
                .unwrap_err()
                .0
                .contains("unsupported parquet logical type Null")
        );
    }
}

#[test]
fn logical_uint32_preserves_high_bits_and_uuid_preserves_bytes() {
    use crate::file_parser::FileParser;
    use parquet::{
        data_type::{FixedLenByteArray, FixedLenByteArrayType, Int32Type},
        file::writer::SerializedFileWriter,
    };
    let schema = Arc::new(parquet::schema::parser::parse_message_type("message schema { REQUIRED INT32 u32 (UINT_32); REQUIRED FIXED_LEN_BYTE_ARRAY(16) uuid (UUID); }").unwrap());
    let mut writer = SerializedFileWriter::new(Vec::new(), schema, Default::default()).unwrap();
    let mut group = writer.next_row_group().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    column
        .typed::<Int32Type>()
        .write_batch(&[0, i32::MAX, i32::MIN, -1], None, None)
        .unwrap();
    column.close().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    column
        .typed::<FixedLenByteArrayType>()
        .write_batch(
            &vec![FixedLenByteArray::from(b"0123456789abcdef".to_vec()); 4],
            None,
            None,
        )
        .unwrap();
    column.close().unwrap();
    group.close().unwrap();
    let mut parser = FileParser::new(logical_parquet_source(writer.into_inner().unwrap())).unwrap();
    for value in [0, 2147483647, 2147483648, 4294967295] {
        assert_eq!(
            parser.read_row().unwrap(),
            vec![
                Datum::UInt(value),
                Datum::Bytes(b"0123456789abcdef".to_vec())
            ]
        );
    }
}

#[test]
fn logical_validation_enforces_scalar_scope_and_physical_encoding() {
    use crate::file_parser::validate_parquet_logical_type as validate;
    use parquet::basic::{LogicalType as L, TimeUnit as U, Type as P};
    for logical in [L::List, L::Map, L::Float16, L::variant(None)] {
        assert!(
            validate(&Some(logical), P::FIXED_LEN_BYTE_ARRAY, 2, "column")
                .unwrap_err()
                .0
                .contains("unsupported parquet logical type")
        );
    }
    for (logical, physical, length) in [
        (L::time(false, U::NANOS), P::INT32, -1),
        (L::decimal(2, 10), P::INT32, -1),
        (L::Uuid, P::FIXED_LEN_BYTE_ARRAY, 15),
        (L::integer(7, true), P::INT32, -1),
    ] {
        assert!(
            validate(&Some(logical), physical, length, "column")
                .unwrap_err()
                .0
                .contains("not applicable")
        );
    }
    // Empty files must still be rejected at schema initialization.
    let schema = Arc::new(parquet::schema::parser::parse_message_type("message schema { OPTIONAL group values (LIST) { REPEATED group list { REQUIRED INT32 element; } } }").unwrap());
    let writer =
        parquet::file::writer::SerializedFileWriter::new(Vec::new(), schema, Default::default())
            .unwrap();
    assert!(
        crate::file_parser::FileParser::new(logical_parquet_source(writer.into_inner().unwrap()))
            .err()
            .unwrap()
            .0
            .contains("nested or repeated")
    );
}

#[test]
fn logical_int64_decimal_signedness_and_timestamp_units_match_go() {
    use crate::file_parser::FileParser;
    for (annotation, value, expected) in [
        ("DECIMAL(18,0)", 0, Datum::Decimal("0".into())),
        ("DECIMAL(18,3)", 0, Datum::Decimal("0.000".into())),
        ("DECIMAL(18,6)", 0, Datum::Decimal("0.000000".into())),
        ("DECIMAL(18,0)", 123, Datum::Decimal("123".into())),
        ("DECIMAL(18,3)", 123, Datum::Decimal("0.123".into())),
        ("DECIMAL(18,0)", -7, Datum::Decimal("-7".into())),
        ("DECIMAL(18,2)", -7, Datum::Decimal("-0.07".into())),
        ("DECIMAL(18,3)", 1, Datum::Decimal("0.001".into())),
        ("DECIMAL(18,1)", 10, Datum::Decimal("1.0".into())),
        ("DECIMAL(18,4)", -1, Datum::Decimal("-0.0001".into())),
        (
            "DECIMAL(18,2)",
            -12345678,
            Datum::Decimal("-123456.78".into()),
        ),
        ("DECIMAL(18,4)", -1, Datum::Decimal("-0.0001".into())),
        ("INT_64", -1, Datum::Int(-1)),
        ("UINT_64", -1, Datum::UInt(u64::MAX)),
        (
            "TIMESTAMP(MILLIS,false)",
            1603963672356,
            Datum::TimeMicros(1603963672356000),
        ),
        (
            "TIMESTAMP(MICROS,false)",
            1603963672356956,
            Datum::TimeMicros(1603963672356956),
        ),
        (
            "TIMESTAMP(MILLIS,true)",
            1603963672356,
            Datum::TimeMicros(1603992472356000),
        ),
        (
            "TIMESTAMP(MICROS,true)",
            1603963672356956,
            Datum::TimeMicros(1603992472356956),
        ),
        ("TIMESTAMP(NANOS,false)", -501, Datum::TimeMicros(-1)),
        ("TIMESTAMP(NANOS,false)", -500, Datum::TimeMicros(0)),
    ] {
        let mut parser = FileParser::new_with_location(
            logical_parquet_source(logical_int64_file(annotation, &[value], None)),
            "Asia/Shanghai",
        )
        .unwrap();
        assert_eq!(parser.read_row().unwrap(), vec![expected], "{annotation}");
    }
}

#[test]
fn logical_null_later_non_null_value_does_not_fail_an_earlier_null_row() {
    let mut parser = crate::file_parser::FileParser::new(logical_parquet_source(
        logical_null_file_rows(parquet::basic::Type::INT32, &[0, 1]),
    ))
    .unwrap();
    assert_eq!(parser.read_row().unwrap(), vec![Datum::Null]);
    assert!(
        parser
            .read_row()
            .unwrap_err()
            .0
            .contains("unsupported parquet logical type Null")
    );
}

#[test]
fn logical_int32_legacy_bridge_preserves_date_decimal_signedness_and_time() {
    use crate::file_parser::{FileParser, ImportParser};
    use astersql_lightning_mydump::{Datum as D, Parser as _};
    use parquet::{data_type::Int32Type, file::writer::SerializedFileWriter};
    for (annotation, value, expected) in [
        ("TIME_MILLIS", 123456, "08:02:03.456000"),
        ("TIME(MILLIS,false)", 123456, "00:02:03.456000"),
        ("DATE", 18564, "2020-10-29"),
        ("DECIMAL(9,2)", -12345678, "-123456.78"),
    ] {
        let schema = Arc::new(
            parquet::schema::parser::parse_message_type(&format!(
                "message schema {{ OPTIONAL INT32 value ({annotation}); }}"
            ))
            .unwrap(),
        );
        let mut writer = SerializedFileWriter::new(Vec::new(), schema, Default::default()).unwrap();
        let mut group = writer.next_row_group().unwrap();
        let mut column = group.next_column().unwrap().unwrap();
        column
            .typed::<Int32Type>()
            .write_batch(&[value], Some(&[1]), None)
            .unwrap();
        column.close().unwrap();
        group.close().unwrap();
        let mut parser = ImportParser::new(
            FileParser::new_with_location(
                logical_parquet_source(writer.into_inner().unwrap()),
                "Asia/Shanghai",
            )
            .unwrap(),
        );
        parser.ReadRow().unwrap();
        assert_eq!(
            parser.LastRow().row,
            vec![D::Bytes(expected.as_bytes().to_vec())],
            "{annotation}"
        );
    }
}

#[test]
fn logical_int96_without_annotation_keeps_utc_adjustment_and_rounding() {
    use crate::file_parser::FileParser;
    use parquet::{
        data_type::{Int96, Int96Type},
        file::writer::SerializedFileWriter,
    };
    let schema = Arc::new(
        parquet::schema::parser::parse_message_type("message schema { REQUIRED INT96 value; }")
            .unwrap(),
    );
    let mut writer = SerializedFileWriter::new(Vec::new(), schema, Default::default()).unwrap();
    let mut group = writer.next_row_group().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    let mut bytes = new_int96(86_399_999_999);
    let nanos = u64::from_le_bytes(bytes[..8].try_into().unwrap()) + 500;
    bytes[..8].copy_from_slice(&nanos.to_le_bytes());
    let mut value = Int96::default();
    value.set_data(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        u32::from_le_bytes(bytes[8..].try_into().unwrap()),
    );
    column
        .typed::<Int96Type>()
        .write_batch(&[value], None, None)
        .unwrap();
    column.close().unwrap();
    group.close().unwrap();
    let mut parser = FileParser::new_with_location(
        logical_parquet_source(writer.into_inner().unwrap()),
        "Asia/Shanghai",
    )
    .unwrap();
    assert_eq!(
        parser.read_row().unwrap(),
        vec![Datum::TimeMicros(115_200_000_000)]
    );
}

#[test]
fn logical_timestamp_nanos_rounding_observes_the_dst_transition_offset() {
    // 2020-03-08 06:59:59.999999500 UTC rounds into New York's 03:00 DST clock.
    let value = 1_583_650_800_000_000_000 - 500;
    let mut parser = crate::file_parser::FileParser::new_with_location(
        logical_parquet_source(logical_int64_file("TIMESTAMP(NANOS,true)", &[value], None)),
        "America/New_York",
    )
    .unwrap();
    assert_eq!(
        parser.read_row().unwrap(),
        vec![Datum::TimeMicros(1_583_636_400_000_000)]
    );
}

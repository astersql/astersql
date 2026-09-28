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
    assert_eq!(parser.scanned_pos(), 13);
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
fn parquet_parser_rejects_unsupported_logical_and_nanos_types() {
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
    assert!(NewParser(nanos).is_err());
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

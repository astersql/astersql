// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// TableRegion 切分与引擎分配相关单元测试。
//
// 验证行号/偏移访问器、AllocateEngineIDs 分桶、大 CSV/自定义终止符切分、
// Parquet/压缩文件 Region 边界，以及 CRLF 中间 seek 等场景。
use crate::test_support::MemoryStorage;
use crate::*;
use std::collections::BTreeMap;

/// 构造指定路径、类型与大小的 SourceFileMeta。
fn meta(path: &str, source_type: SourceType, size: i64) -> SourceFileMeta {
    SourceFileMeta {
        path: path.into(),
        source_type,
        file_size: size,
        real_size: size,
        ..Default::default()
    }
}

/// 返回带固定 chunk 区间的样例 TableRegion。
fn region() -> TableRegion {
    TableRegion {
        engine_id: 0,
        db: "db".into(),
        table: "t".into(),
        file_meta: meta("t.csv", SourceType::Csv, 100),
        extend_data: ExtendColumnData::default(),
        chunk: Chunk {
            offset: 10,
            end_offset: 40,
            prev_row_id_max: 4,
            row_id_max: 11,
            ..Default::default()
        },
    }
}

#[test]
/// 校验 RowIDMin/Rows/Offset/Size 访问器。
fn TestTableRegion() {
    let region = region();
    assert_eq!(region.RowIDMin(), 5);
    assert_eq!(region.Rows(), 7);
    assert_eq!(region.Offset(), 10);
    assert_eq!(region.Size(), 30);
}

#[test]
/// 总量小于 batch 时全归引擎 0；否则按 ratio/concurrency 分桶计数。
fn TestAllocateEngineIDs() {
    let sizes = vec![1.0; 700];
    let mut regions = vec![region(); 700];
    AllocateEngineIDs(&mut regions, &sizes, 1000.0, 0.5, 1000.0);
    assert!(regions.iter().all(|region| region.engine_id == 0));

    for (batch, ratio, concurrency, expected) in [
        (200.0, 0.5, 1000.0, vec![(0, 170), (1, 213), (2, 317)]),
        (200.0, 0.6, 1000.0, vec![(0, 160), (1, 208), (2, 332)]),
        (
            100.0,
            0.5,
            1000.0,
            vec![(0, 93), (1, 105), (2, 122), (3, 153), (4, 227)],
        ),
        (
            50.0,
            0.5,
            4.0,
            vec![
                (0, 50),
                (1, 59),
                (2, 73),
                (3, 110),
                (4, 50),
                (5, 50),
                (6, 50),
                (7, 50),
                (8, 50),
                (9, 50),
                (10, 50),
                (11, 50),
                (12, 8),
            ],
        ),
        (
            100.0,
            0.0,
            1000.0,
            vec![
                (0, 100),
                (1, 100),
                (2, 100),
                (3, 100),
                (4, 100),
                (5, 100),
                (6, 100),
            ],
        ),
    ] {
        AllocateEngineIDs(&mut regions, &sizes, batch, ratio, concurrency);
        let mut counts = BTreeMap::new();
        for region in &regions {
            *counts.entry(region.engine_id).or_insert(0) += 1;
        }
        assert_eq!(counts.into_iter().collect::<Vec<_>>(), expected);
    }
}

#[test]
/// 非正 batch 使用 Go 默认 100 GiB；正值不得因行序或总量被重写。
fn TestCalculateBatchSizeMatchesGoDefaults() {
    const DEFAULT_BATCH_SIZE: f64 = (100_i64 * 1024 * 1024 * 1024) as f64;
    assert_eq!(CalculateBatchSize(256.0, true, 1_000.0), 256.0);
    assert_eq!(CalculateBatchSize(256.0, false, 1_000.0), 256.0);
    assert_eq!(CalculateBatchSize(0.0, true, 1_000.0), DEFAULT_BATCH_SIZE);
    assert_eq!(
        CalculateBatchSize(0.0, false, DEFAULT_BATCH_SIZE * 2.0),
        DEFAULT_BATCH_SIZE * 2.0
    );
}

/// 在内存存储中对 data.csv 调用 SplitLargeCSV 的便捷包装。
fn split(input: &[u8], mut cfg: DataDivideConfig) -> Vec<TableRegion> {
    let store = MemoryStorage::with(&[("data.csv", input)]);
    let source = meta("data.csv", SourceType::Csv, input.len() as i64);
    cfg.csv.fields_enclosed_by.clear();
    if cfg.column_count == 0 {
        cfg.column_count = 1;
    }
    SplitLargeCSV(&source, &cfg, &store).unwrap()
}

#[test]
/// MakeTableRegions 对带头 CSV 按 region_size=1 切出多个分片。
fn TestMakeTableRegionsSplitLargeFile() {
    let input = b"a,b,c\n1,2,3\n4,5,6\n7,8,9\n0,1,2\n";
    let store = MemoryStorage::with(&[("data.csv", input)]);
    let mut table = NewMDTableMeta("auto");
    table.db = "csv".into();
    table.name = "large".into();
    table.total_size = input.len() as i64;
    table.data_files.push(FileInfo {
        file_meta: FileMeta {
            path: "data.csv".into(),
            file_size: input.len() as i64,
            source_type: SourceType::Csv,
            ..Default::default()
        },
        ..Default::default()
    });
    let mut cfg = NewDataDivideConfig();
    cfg.column_count = 3;
    cfg.region_size = 1;
    cfg.strict_format = true;
    cfg.csv.header = true;
    cfg.csv.fields_enclosed_by.clear();
    let regions = MakeTableRegions(&table, &cfg, &store).unwrap();
    assert_eq!(regions.len(), 4);
    assert_eq!(regions[0].chunk.offset, 6);
    assert_eq!(regions[0].chunk.columns, vec!["a", "b", "c"]);
    assert_eq!(regions.last().unwrap().chunk.end_offset, input.len() as i64);
}

#[test]
/// Parquet 用 real_size 参与引擎分配，且 end_offset 为 i64::MAX。
fn TestParquetFileRegionUsesRealSizeForEngineAllocation() {
    let sources = [
        SourceFileMeta {
            path: "a.parquet".into(),
            source_type: SourceType::Parquet,
            file_size: 40,
            real_size: 400,
            rows: 100,
            ..Default::default()
        },
        SourceFileMeta {
            path: "b.parquet".into(),
            source_type: SourceType::Parquet,
            file_size: 40,
            real_size: 400,
            rows: 100,
            ..Default::default()
        },
        SourceFileMeta {
            path: "c.parquet".into(),
            source_type: SourceType::Parquet,
            file_size: 40,
            real_size: 400,
            rows: 100,
            ..Default::default()
        },
    ];
    let mut regions = sources
        .iter()
        .map(|source| makeParquetFileRegion(source, 0))
        .collect::<Vec<_>>();
    AllocateEngineIDs(&mut regions, &[400.0; 3], 200.0, 0.75, 10.0);
    assert!(regions.iter().map(|region| region.engine_id).max().unwrap() > 0);
    assert!(
        regions
            .iter()
            .all(|region| region.chunk.end_offset == i64::MAX)
    );
}

#[test]
/// 压缩 CSV 的 end_offset 为 INF，行号按 CompressSizeFactor 放大。
fn TestCompressedMakeSourceFileRegion() {
    let source = SourceFileMeta {
        path: "data.csv.zst".into(),
        source_type: SourceType::Csv,
        compression: Compression::Zstd,
        file_size: 40,
        real_size: 400,
        ..Default::default()
    };
    let region = MakeSourceFileRegion(&source, 0, 3);
    assert_eq!(region.chunk.offset, 0);
    assert_eq!(region.chunk.end_offset, TableFileSizeINF);
    assert_eq!(region.chunk.row_id_max, 400 * CompressSizeFactor / 3);
}

#[test]
/// Go 使用列数估算 CSV 行号，非 CSV 还需计入两列内部开销。
fn TestMakeSourceFileRegionUsesColumnDivisor() {
    let csv = meta("data.csv", SourceType::Csv, 120);
    assert_eq!(MakeSourceFileRegion(&csv, 7, 3).chunk.row_id_max, 47);

    let sql = meta("data.sql", SourceType::Sql, 120);
    assert_eq!(MakeSourceFileRegion(&sql, 7, 3).chunk.row_id_max, 31);
}

#[test]
/// 与 Go 一致：非 strict CSV 以及仅轻微超过 region_size 的文件均不拆分。
fn TestMakeTableRegionsHonorsStrictAndLargeThreshold() {
    let input = b"a,b\n1,2\n3,4\n";
    let store = MemoryStorage::with(&[("data.csv", input)]);
    let mut table = NewMDTableMeta("auto");
    table.data_files.push(FileInfo {
        file_meta: FileMeta {
            path: "data.csv".into(),
            file_size: input.len() as i64,
            real_size: input.len() as i64,
            source_type: SourceType::Csv,
            ..Default::default()
        },
        ..Default::default()
    });

    let mut cfg = NewDataDivideConfig();
    cfg.column_count = 2;
    cfg.region_size = 10;
    assert_eq!(MakeTableRegions(&table, &cfg, &store).unwrap().len(), 1);

    cfg.strict_format = true;
    table.data_files[0].file_meta.file_size = 11;
    assert_eq!(MakeTableRegions(&table, &cfg, &store).unwrap().len(), 1);
}

#[test]
/// 多文件按原始顺序重基 row-id，且整文件引擎分配使用 Go 返回的 real_size。
fn TestMakeTableRegionsRebasesRowsAndUsesRealSize() {
    let store = MemoryStorage::default();
    let mut table = NewMDTableMeta("auto");
    table.is_row_ordered = true;
    table.total_size = 800;
    for path in ["a.sql", "b.sql"] {
        table.data_files.push(FileInfo {
            file_meta: FileMeta {
                path: path.into(),
                file_size: 30,
                real_size: 400,
                source_type: SourceType::Sql,
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let mut cfg = NewDataDivideConfig();
    cfg.column_count = 1;
    cfg.engine_data_size = 200.0;
    let regions = MakeTableRegions(&table, &cfg, &store).unwrap();
    assert_eq!(regions[0].chunk.row_id_max, 10);
    assert_eq!(regions[1].chunk.prev_row_id_max, 10);
    assert_eq!(regions[1].chunk.row_id_max, 20);
    assert!(regions[1].engine_id > regions[0].engine_id);
}

#[test]
/// Go 在打开 CSV parser 时传播非法字符集配置错误。
fn TestSplitLargeCsvRejectsInvalidCharset() {
    let input = b"a,b\n1,2\n";
    let store = MemoryStorage::with(&[("data.csv", input)]);
    let source = meta("data.csv", SourceType::Csv, input.len() as i64);
    let mut cfg = NewDataDivideConfig();
    cfg.charset = "not-a-charset".into();
    let error = SplitLargeCSV(&source, &cfg, &store).unwrap_err();
    assert!(error.to_string().contains("unknown charset"));
}

#[test]
/// 基本切分：首分片跳过表头，相邻分片 end==next.offset。
fn TestSplitLargeFile() {
    let mut cfg = NewDataDivideConfig();
    cfg.region_size = 6;
    cfg.csv.header = true;
    let regions = split(b"a,b,c\n1,2,3\n4,5,6\n7,8,9\n0,1,2\n", cfg);
    assert_eq!(
        regions
            .iter()
            .map(|region| (region.chunk.offset, region.chunk.end_offset))
            .collect::<Vec<_>>(),
        vec![(6, 18), (18, 24), (24, 30)]
    );
}

#[test]
/// 文件末尾无换行时，最后分片 end_offset 仍等于文件长度。
fn TestSplitLargeFileNoNewLineAtEOF() {
    let mut cfg = NewDataDivideConfig();
    cfg.region_size = 1;
    cfg.csv.header = true;
    cfg.csv.lines_terminated_by.clear();
    let input = b"a,b\r\n123,456\r\n789,101";
    let regions = split(input, cfg);
    assert_eq!(
        regions
            .iter()
            .map(|region| (region.chunk.offset, region.chunk.end_offset))
            .collect::<Vec<_>>(),
        vec![(4, 13), (13, 14), (14, 21)]
    );
}

#[test]
/// 自定义字段/行终止符时仍能正确切出多个分片。
fn TestSplitLargeFileWithCustomTerminator() {
    let mut cfg = NewDataDivideConfig();
    cfg.region_size = 1;
    cfg.csv.fields_terminated_by = "|+|".into();
    cfg.csv.lines_terminated_by = "|+|\n".into();
    let input = b"5|+|abc\ndef\nghi|+|6|+|\n7|+|xyz|+|8|+|\n9|+||+|10";
    let regions = split(input, cfg);
    assert_eq!(
        regions
            .iter()
            .map(|region| (region.chunk.offset, region.chunk.end_offset))
            .collect::<Vec<_>>(),
        vec![(0, 23), (23, 38), (38, 47)]
    );
}

#[test]
/// region_size 足够大时只产生一个分片并保留表头列名。
fn TestSplitLargeFileOnlyOneChunk() {
    let mut cfg = NewDataDivideConfig();
    cfg.region_size = 15;
    cfg.csv.header = true;
    cfg.csv.lines_terminated_by = "\r\n".into();
    let regions = split(b"field1,field2\r\n123,456\r\n", cfg);
    assert_eq!(
        (regions[0].chunk.offset, regions[0].chunk.end_offset),
        (14, 24)
    );
    assert_eq!(regions[0].chunk.columns, vec!["field1", "field2"]);
}

#[test]
/// 切分点落在 CRLF 中间时仍保持行数与尾偏移正确。
fn TestSplitLargeFileSeekInsideCRLF() {
    let mut cfg = NewDataDivideConfig();
    cfg.region_size = 2;
    cfg.csv.lines_terminated_by = "\r\n".into();
    let input = b"1\r\n2\r\n3\r\n4\r\n";
    let regions = split(input, cfg);
    assert_eq!(
        regions
            .iter()
            .map(|region| (region.chunk.offset, region.chunk.end_offset))
            .collect::<Vec<_>>(),
        vec![(0, 6), (6, 9), (9, 12)]
    );
    assert_eq!(
        regions.iter().map(|region| region.Rows()).sum::<i64>(),
        input.len() as i64
    );
    assert_eq!(regions.last().unwrap().chunk.end_offset, input.len() as i64);
}

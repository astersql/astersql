// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL 扫描范围按 Region（TiKV 数据分片）切分的单元测试。
//
// 覆盖有符号/无符号/字节主键、Region 不可用或合并时的退化行为，以及句柄（handle，
// 行标识）边界解码辅助函数，对应 Go `split_test.go` 中的切分逻辑。

use crate::table::{
    Column, GetASCIIPrefixDatumFromBytes, GetNextBytesHandleDatum, GetNextIntDatumFromCommonHandle,
    GetNextIntHandle, KeyKind, KeyRange, NewPhysicalTable, RegionProvider, TTLInfo, TableInfo,
    TimeUnit,
};
use crate::task::Datum;

// record_prefix_for_test 独立复现 `crate::table::record_prefix` 的编码方案（该函数是私有的），
// 只为测试构造落在同一 keyspace 内的 region 边界；两侧算法必须保持一致，否则下面的断言会失败，
// 这本身就能在算法漂移时充当回归检测。
fn record_prefix_for_test(table_id: i64) -> Vec<u8> {
    let mut prefix = vec![b't'];
    prefix.extend_from_slice(&((table_id as u64) ^ (1 << 63)).to_be_bytes());
    prefix.extend_from_slice(b"_r");
    prefix
}
/// 构造整数句柄对应的完整行键：`record_prefix || memcomparable(handle)`。
fn int_handle_key(table_id: i64, handle: i64) -> Vec<u8> {
    let mut key = record_prefix_for_test(table_id);
    key.extend_from_slice(&((handle as u64) ^ (1 << 63)).to_be_bytes());
    key
}
/// 构造字节句柄行键：在表前缀后直接追加 suffix。
fn bytes_handle_key(table_id: i64, suffix: &[u8]) -> Vec<u8> {
    let mut key = record_prefix_for_test(table_id);
    key.extend_from_slice(suffix);
    key
}

/// 按主键列类型构造带 TTL 的 `PhysicalTable`（整数句柄或 common handle）。
fn ttl_physical_table(table_id: i64, key_column: Column) -> crate::table::PhysicalTable {
    let is_int_handle = matches!(
        key_column.key_kind,
        KeyKind::SignedInt | KeyKind::UnsignedInt
    );
    let table = TableInfo {
        id: table_id,
        name: "t".into(),
        public: true,
        pk_is_handle: is_int_handle,
        common_handle: !is_int_handle,
        columns: vec![
            key_column,
            Column {
                id: 2,
                name: "t".into(),
                public: true,
                key_kind: KeyKind::SignedInt,
                nullable: false,
                hidden: false,
            },
        ],
        primary_index_offsets: if is_int_handle { Vec::new() } else { vec![0] },
        indexes: Vec::new(),
        partitions: Vec::new(),
        ttl: Some(TTLInfo {
            column_name: "t".into(),
            interval: "1".into(),
            unit: TimeUnit::Day,
        }),
    };
    NewPhysicalTable("test", &table, "").unwrap()
}

/// 固定返回预置 Region 列表的 mock。
struct FixedRegions(Vec<KeyRange>);
impl RegionProvider for FixedRegions {
    fn locate_key_range(&self, _start: &[u8], _end: &[u8]) -> Result<Vec<KeyRange>, String> {
        Ok(self.0.clone())
    }
}
/// 返回空 Region 列表，触发全表范围退化。
struct EmptyRegions;
impl RegionProvider for EmptyRegions {
    fn locate_key_range(&self, _start: &[u8], _end: &[u8]) -> Result<Vec<KeyRange>, String> {
        Ok(Vec::new())
    }
}
/// 模拟 Region 缓存不可用，应向上传播错误。
struct FailingRegions;
impl RegionProvider for FailingRegions {
    fn locate_key_range(&self, _start: &[u8], _end: &[u8]) -> Result<Vec<KeyRange>, String> {
        Err("region cache unavailable".into())
    }
}

/// 有符号整数主键列。
fn signed_key(name: &str) -> Column {
    Column {
        id: 1,
        name: name.into(),
        public: true,
        key_kind: KeyKind::SignedInt,
        nullable: false,
        hidden: false,
    }
}
/// 无符号整数主键列。
fn unsigned_key(name: &str) -> Column {
    Column {
        id: 1,
        name: name.into(),
        public: true,
        key_kind: KeyKind::UnsignedInt,
        nullable: false,
        hidden: false,
    }
}
/// 字节串主键列（common handle）。
fn bytes_key(name: &str) -> Column {
    Column {
        id: 1,
        name: name.into(),
        public: true,
        key_kind: KeyKind::Bytes,
        nullable: false,
        hidden: false,
    }
}

// 对应 Go TestSplitTTLScanRangesWithSignedInt 的第一段：region provider 不可用/没有 region 时，
// SplitScanRanges 应退化为覆盖整表的单个 ScanRange。
#[test]
fn test_split_scan_ranges_no_region_falls_back_to_full_range() {
    let table = ttl_physical_table(1, signed_key("id"));
    let ranges = table.SplitScanRanges(Some(&EmptyRegions), 4).unwrap();
    assert_eq!(ranges.len(), 1);
    assert!(ranges[0].Start.is_empty() && ranges[0].End.is_empty());
}

// 对应 Go 中共享 region 的场景：只有一个 region 覆盖当前表时不产生切分。
#[test]
fn test_split_scan_ranges_single_region_falls_back_to_full_range() {
    let table = ttl_physical_table(1, signed_key("id"));
    let regions = FixedRegions(vec![KeyRange {
        start: record_prefix_for_test(0),
        end: record_prefix_for_test(2),
    }]);
    let ranges = table.SplitScanRanges(Some(&regions), 4).unwrap();
    assert_eq!(ranges.len(), 1);
    assert!(ranges[0].Start.is_empty() && ranges[0].End.is_empty());
}

// 对应 Go TestSplitTTLScanRangesWithSignedInt 中多 region 的场景：4 个 region 应切出
// [None,100) [100,200) [200,300) [300,None) 四段有序范围。
#[test]
fn test_split_scan_ranges_with_signed_int_multiple_regions() {
    let table_id = 42;
    let table = ttl_physical_table(table_id, signed_key("id"));
    let boundaries = [100_i64, 200, 300];
    let prefix = record_prefix_for_test(table_id);
    let mut edges = vec![prefix.clone()];
    edges.extend(boundaries.iter().map(|h| int_handle_key(table_id, *h)));
    edges.push(int_handle_key(table_id, i64::MAX));
    let regions: Vec<KeyRange> = edges
        .windows(2)
        .map(|pair| KeyRange {
            start: pair[0].clone(),
            end: pair[1].clone(),
        })
        .collect();
    let ranges = table
        .SplitScanRanges(Some(&FixedRegions(regions)), 4)
        .unwrap();

    assert_eq!(ranges.len(), 4);
    assert!(ranges[0].Start.is_empty());
    assert_eq!(ranges[0].End, vec![Datum::Int(100)]);
    assert_eq!(ranges[1].Start, vec![Datum::Int(100)]);
    assert_eq!(ranges[1].End, vec![Datum::Int(200)]);
    assert_eq!(ranges[2].Start, vec![Datum::Int(200)]);
    assert_eq!(ranges[2].End, vec![Datum::Int(300)]);
    assert_eq!(ranges[3].Start, vec![Datum::Int(300)]);
    assert!(ranges[3].End.is_empty());
}

// 对应 Go TestSplitTTLScanRangesWithUnsignedInt：原始 handle 仍按有符号顺序编码，
// 无符号扫描顺序需把负半区旋转到 u64 高半区。
#[test]
fn test_split_scan_ranges_with_unsigned_int_multiple_regions() {
    let table_id = 43;
    let table = ttl_physical_table(table_id, unsigned_key("id"));
    let boundaries = [10_i64, 20];
    let prefix = record_prefix_for_test(table_id);
    let mut edges = vec![prefix.clone()];
    edges.extend(boundaries.iter().map(|h| int_handle_key(table_id, *h)));
    edges.push(int_handle_key(table_id, i64::MAX));
    let regions: Vec<KeyRange> = edges
        .windows(2)
        .map(|pair| KeyRange {
            start: pair[0].clone(),
            end: pair[1].clone(),
        })
        .collect();
    let ranges = table
        .SplitScanRanges(Some(&FixedRegions(regions)), 3)
        .unwrap();

    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[0].Start, vec![Datum::UInt(i64::MAX as u64 + 1)]);
    assert!(ranges[0].End.is_empty());
    assert!(ranges[1].Start.is_empty());
    assert_eq!(ranges[1].End, vec![Datum::UInt(10)]);
    assert_eq!(ranges[2].Start, vec![Datum::UInt(10)]);
    assert_eq!(ranges[2].End, vec![Datum::UInt(20)]);
    assert_eq!(ranges[3].Start, vec![Datum::UInt(20)]);
    assert_eq!(ranges[3].End, vec![Datum::UInt(i64::MAX as u64 + 1)]);
}

// 对应 Go TestSplitTTLScanRangesWithUnsignedInt 的跨零点场景。TiDB 的整数
// handle 仍按有符号顺序编码，但无符号 Datum 必须在零点拆开并旋转为：
// [2^63, MaxUint64-199) ... [MaxUint64-99, +inf)，随后再覆盖
// (-inf, 100) ... [200, 2^63)。这可防止无符号扫描遗漏高半区。
#[test]
fn test_split_scan_ranges_with_unsigned_int_crosses_zero() {
    let table_id = 47;
    let table = ttl_physical_table(table_id, unsigned_key("id"));
    let prefix = record_prefix_for_test(table_id);
    let boundaries = [-200_i64, -100, 0, 100, 200];
    let mut edges = vec![prefix.clone()];
    edges.extend(
        boundaries
            .iter()
            .map(|handle| int_handle_key(table_id, *handle)),
    );
    edges.push(int_handle_key(table_id, i64::MAX));
    let regions = edges
        .windows(2)
        .map(|pair| KeyRange {
            start: pair[0].clone(),
            end: pair[1].clone(),
        })
        .collect::<Vec<_>>();

    let ranges = table
        .SplitScanRanges(Some(&FixedRegions(regions)), 6)
        .unwrap();

    assert_eq!(ranges.len(), 6);
    assert_eq!(ranges[0].Start, vec![Datum::UInt(i64::MAX as u64 + 1)]);
    assert_eq!(ranges[0].End, vec![Datum::UInt(u64::MAX - 199)]);
    assert_eq!(ranges[1].Start, vec![Datum::UInt(u64::MAX - 199)]);
    assert_eq!(ranges[1].End, vec![Datum::UInt(u64::MAX - 99)]);
    assert_eq!(ranges[2].Start, vec![Datum::UInt(u64::MAX - 99)]);
    assert!(ranges[2].End.is_empty());
    assert!(ranges[3].Start.is_empty());
    assert_eq!(ranges[3].End, vec![Datum::UInt(100)]);
    assert_eq!(ranges[4].Start, vec![Datum::UInt(100)]);
    assert_eq!(ranges[4].End, vec![Datum::UInt(200)]);
    assert_eq!(ranges[5].Start, vec![Datum::UInt(200)]);
    assert_eq!(ranges[5].End, vec![Datum::UInt(i64::MAX as u64 + 1)]);
}

// 对应 Go TestSplitTTLScanRangesWithBytes：字节主键按 region 边界的下一个前缀切分，
// 每个中间边界都是上一个字节串的 prefix-next。
#[test]
fn test_split_scan_ranges_with_bytes_multiple_regions() {
    let table_id = 44;
    let table = ttl_physical_table(table_id, bytes_key("id"));
    let prefix = record_prefix_for_test(table_id);
    // 挑选没有“回退跳变”的 suffix：next(suffix1) < next(suffix2) 才能产生 3 段有效范围；
    // 若 suffix 较短的一侧 +1 后反而越过较长 suffix 的边界，中间段会被算法按设计跳过
    // （对应 Go 原注释 "curScanStart >= curScanEnd because the edge datum is an approximate value"）。
    let suffixes: [&[u8]; 2] = [&[1, 2, 3], &[5, 0, 0]];
    let mut edges = vec![prefix.clone()];
    edges.extend(suffixes.iter().map(|s| bytes_handle_key(table_id, s)));
    edges.push(bytes_handle_key(table_id, &[0xff]));
    let regions: Vec<KeyRange> = edges
        .windows(2)
        .map(|pair| KeyRange {
            start: pair[0].clone(),
            end: pair[1].clone(),
        })
        .collect();
    let ranges = table
        .SplitScanRanges(Some(&FixedRegions(regions)), 3)
        .unwrap();

    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[0].End, vec![Datum::Bytes(vec![1, 2, 4])]);
    assert_eq!(ranges[1].Start, vec![Datum::Bytes(vec![1, 2, 4])]);
    assert_eq!(ranges[1].End, vec![Datum::Bytes(vec![5, 0, 1])]);
    assert_eq!(ranges[2].Start, vec![Datum::Bytes(vec![5, 0, 1])]);
    assert!(ranges[2].End.is_empty());
}

// 对应 Go 注释 "Sometimes curScanStart >= curScanEnd because the edge datum is an approximate
// value. At this time, we should skip this range"：当较短 suffix 递增后的值反超较长 suffix
// 递增后的值时，中间那段范围会被跳过，只保留首尾两段。
#[test]
fn test_split_scan_ranges_with_bytes_skips_out_of_order_middle_edge() {
    let table_id = 46;
    let table = ttl_physical_table(table_id, bytes_key("id"));
    let prefix = record_prefix_for_test(table_id);
    let suffixes: [&[u8]; 2] = [&[1, 2, 3], &[1, 2, 3, 4, 5]];
    let mut edges = vec![prefix.clone()];
    edges.extend(suffixes.iter().map(|s| bytes_handle_key(table_id, s)));
    edges.push(bytes_handle_key(table_id, &[0xff]));
    let regions: Vec<KeyRange> = edges
        .windows(2)
        .map(|pair| KeyRange {
            start: pair[0].clone(),
            end: pair[1].clone(),
        })
        .collect();
    let ranges = table
        .SplitScanRanges(Some(&FixedRegions(regions)), 3)
        .unwrap();

    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0].End, vec![Datum::Bytes(vec![1, 2, 4])]);
    // 中间段被跳过后，扫描游标仍然前进到被跳过的 end（而不是回退），
    // 因此下一段的起点是 next(suffix2)，不是 next(suffix1)。
    assert_eq!(ranges[1].Start, vec![Datum::Bytes(vec![1, 2, 3, 4, 6])]);
    assert!(ranges[1].End.is_empty());
}

// 对应 Go TestNoTTLSplitSupportTables：split_count<=1 或缺少 key column 时不应尝试切分。
#[test]
fn test_split_scan_ranges_respects_split_count_guard() {
    let table = ttl_physical_table(1, signed_key("id"));
    let regions = FixedRegions(vec![
        KeyRange {
            start: vec![0],
            end: vec![1],
        },
        KeyRange {
            start: vec![1],
            end: vec![2],
        },
    ]);
    assert_eq!(table.SplitScanRanges(Some(&regions), 1).unwrap().len(), 1);
    assert_eq!(table.SplitScanRanges(Some(&regions), 0).unwrap().len(), 1);
}

// 对应 Go：region provider 返回错误时应向上传播，而不是静默退化为全表扫描。
#[test]
fn test_split_scan_ranges_propagates_region_provider_error() {
    let table = ttl_physical_table(1, signed_key("id"));
    assert!(table.SplitScanRanges(Some(&FailingRegions), 4).is_err());
}

// 对应 Go TestMergeRegion/TestRegionDisappearDuringSplitRange 的核心意图：region 集合在两次
// 调用之间发生变化（模拟并发合并）时，SplitScanRanges 应始终成功返回、不 panic，且
// 返回的分段数不超过可用的 region 数。
#[test]
fn test_split_scan_ranges_tolerates_region_count_changing_between_calls() {
    let table_id = 45;
    let table = ttl_physical_table(table_id, signed_key("id"));
    let prefix = record_prefix_for_test(table_id);

    let many_edges: Vec<Vec<u8>> = std::iter::once(prefix.clone())
        .chain((0..8).map(|i| int_handle_key(table_id, i * 100)))
        .chain(std::iter::once(int_handle_key(table_id, i64::MAX)))
        .collect();
    let many_regions: Vec<KeyRange> = many_edges
        .windows(2)
        .map(|pair| KeyRange {
            start: pair[0].clone(),
            end: pair[1].clone(),
        })
        .collect();
    let ranges = table
        .SplitScanRanges(Some(&FixedRegions(many_regions.clone())), 16)
        .unwrap();
    assert!(ranges.len() <= many_regions.len());

    // 模拟一次合并：region 数量减半。
    let merged_regions: Vec<KeyRange> = many_regions
        .chunks(2)
        .map(|pair| KeyRange {
            start: pair[0].start.clone(),
            end: pair.last().unwrap().end.clone(),
        })
        .collect();
    let ranges_after_merge = table
        .SplitScanRanges(Some(&FixedRegions(merged_regions.clone())), 16)
        .unwrap();
    assert!(ranges_after_merge.len() <= merged_regions.len());
}

// 对应 Go TestGetNextIntHandle：验证签名整数句柄的编码/解码边界行为。
#[test]
fn test_get_next_int_handle_boundaries() {
    let prefix = record_prefix_for_test(7);

    // 恰好等于 prefix 或更短：回到 handle 空间的最小值。
    assert_eq!(GetNextIntHandle(&prefix, &prefix), Some(i64::MIN));
    assert_eq!(
        GetNextIntHandle(&prefix[..prefix.len() - 1], &prefix),
        Some(i64::MIN)
    );

    // 不以 prefix 为前缀，且字典序大于 prefix：属于下一张表，返回 None。
    let mut other_table = prefix.clone();
    *other_table.last_mut().unwrap() += 1;
    assert!(GetNextIntHandle(&other_table, &prefix).is_none());

    // 恰好 8 字节 suffix：精确解码出写入的 handle 值。
    assert_eq!(GetNextIntHandle(&int_handle_key(7, 0), &prefix), Some(0));
    assert_eq!(
        GetNextIntHandle(&int_handle_key(7, 12345), &prefix),
        Some(12345)
    );
    assert_eq!(
        GetNextIntHandle(&int_handle_key(7, i64::MAX), &prefix),
        Some(i64::MAX)
    );
    assert_eq!(
        GetNextIntHandle(&int_handle_key(7, i64::MIN), &prefix),
        Some(i64::MIN)
    );

    // 超过 8 字节 suffix：解码出下一个可能的 handle（+1），MAX 时溢出为 None。
    let mut extra = int_handle_key(7, 7);
    extra.push(0);
    assert_eq!(GetNextIntHandle(&extra, &prefix), Some(8));
    let mut overflow = int_handle_key(7, i64::MAX);
    overflow.push(0);
    assert!(GetNextIntHandle(&overflow, &prefix).is_none());
}

// 对应 Go TestGetNextIntDatumFromCommonHandle：无符号语义只是把同一套整数编码强转为 u64。
#[test]
fn test_get_next_int_datum_from_common_handle() {
    let prefix = record_prefix_for_test(7);
    assert_eq!(
        GetNextIntDatumFromCommonHandle(&int_handle_key(7, 0), &prefix, true),
        Datum::UInt(0)
    );
    assert_eq!(
        GetNextIntDatumFromCommonHandle(&int_handle_key(7, 1024), &prefix, true),
        Datum::UInt(1024)
    );
    assert_eq!(
        GetNextIntDatumFromCommonHandle(&int_handle_key(7, -1), &prefix, false),
        Datum::Int(-1)
    );
    let mut other_table = prefix.clone();
    *other_table.last_mut().unwrap() += 1;
    assert_eq!(
        GetNextIntDatumFromCommonHandle(&other_table, &prefix, true),
        Datum::Null
    );
}

// 对应 Go TestGetNextBytesHandleDatum：字节句柄解码返回“下一个更大”的字节串。
#[test]
fn test_get_next_bytes_handle_datum() {
    let prefix = record_prefix_for_test(7);
    assert_eq!(
        GetNextBytesHandleDatum(&prefix, &prefix),
        Datum::Bytes(Vec::new())
    );
    assert_eq!(
        GetNextBytesHandleDatum(&bytes_handle_key(7, &[1, 2, 3]), &prefix),
        Datum::Bytes(vec![1, 2, 4])
    );
    assert_eq!(
        GetNextBytesHandleDatum(&bytes_handle_key(7, &[1, 2, 0xff]), &prefix),
        Datum::Bytes(vec![1, 3])
    );
    let mut other_table = prefix.clone();
    *other_table.last_mut().unwrap() += 1;
    assert_eq!(GetNextBytesHandleDatum(&other_table, &prefix), Datum::Null);
}

// 对应 Go TestGetASCIIPrefixDatumFromBytes：逐个字符类别的可见性判断。
#[test]
fn test_get_ascii_prefix_datum_from_bytes_table_driven() {
    let cases: &[(&[u8], &str)] = &[
        (b"", ""),
        (&[0], ""),
        (&[9], "\t"),
        (&[10], "\n"),
        (&[13], "\r"),
        (&[0x20], " "),
        (&[0x7e], "~"),
        (&[0x7f], ""),
        (&[0xff], ""),
        (b"ab\rcd\tef\nAB!~GH()tt ;;", "ab\rcd\tef\nAB!~GH()tt ;;"),
        ("中文".as_bytes(), ""),
        ("cn中文".as_bytes(), "cn"),
        ("emoji\u{1F600}".as_bytes(), "emoji"),
    ];
    for (bytes, expected) in cases {
        assert_eq!(
            GetASCIIPrefixDatumFromBytes(bytes),
            Datum::String((*expected).to_owned())
        );
    }
}

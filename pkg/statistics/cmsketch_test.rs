// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// CMSketch 与 TopN 的单元测试：构建、查询、合并、编解码与排序。
//
// 覆盖 Count-Min Sketch 频数估计、高频值 TopN 合并溢出到 sketch，以及 protobuf 往返。

use crate::*;

#[test]
/// 允许 depth=1、width=1 的最小合法 sketch，内存占用为 4 字节。
fn TestNewCMSketchAllowsSingleCounterWidth() {
    let sketch = NewCMSketch(1, 1);
    assert_eq!(sketch.MemoryUsage(), 4);
}

#[test]
/// 从样本构建 CMSketch+TopN，并验证 protobuf 往返后内容一致。
fn cmsketch_and_topn_build_query_merge_and_round_trip() {
    let samples = vec![
        b"a".to_vec(),
        b"a".to_vec(),
        b"a".to_vec(),
        b"b".to_vec(),
        b"b".to_vec(),
        b"c".to_vec(),
    ];
    let (sketch, top_n, ndv, ratio) = NewCMSketchAndTopN(5, 128, &samples, 2, 60);
    let sketch = sketch.unwrap();
    assert_eq!(ndv, 6.min(ndv));
    assert_eq!(ratio, 10);
    assert!(top_n.as_ref().is_some_and(|top_n| top_n.Num() >= 1));

    let proto = CMSketchToProto(Some(&sketch), top_n.as_ref());
    let (decoded, decoded_top_n) = CMSketchAndTopNFromProto(Some(&proto));
    assert!(decoded.unwrap().Equal(&sketch));
    assert_eq!(decoded_top_n.unwrap(), top_n.unwrap());
}

#[test]
/// MergeTopN 合并相同键的计数，并按频率把溢出项 spill 出去。
fn merge_topn_coalesces_equal_values_and_spills_by_frequency() {
    let mut left = NewTopN(2);
    left.AppendTopN(b"a".to_vec(), 2);
    left.AppendTopN(b"b".to_vec(), 3);
    left.Sort();
    let mut right = NewTopN(2);
    right.AppendTopN(b"a".to_vec(), 5);
    right.AppendTopN(b"c".to_vec(), 4);
    right.Sort();
    let (merged, spilled) = MergeTopN(&[&left, &right], 2);
    let merged = merged.unwrap();
    assert_eq!(merged.QueryTopN(b"a"), (7, true));
    assert_eq!(merged.QueryTopN(b"c"), (4, true));
    assert_eq!(
        spilled,
        vec![TopNMeta {
            Encoded: b"b".to_vec(),
            Count: 3
        }]
    );
}

// Go TestCMSketch.
/// 对应 Go TestCMSketch：插入、按哈希扣减，以及维度不匹配时 Merge 失败。
#[test]
fn cmsketch_insert_subtract_and_dimension_mismatch() {
    let mut sketch = NewCMSketch(5, 128);
    sketch.InsertBytesByCount(b"x", 8);
    assert_eq!(sketch.QueryBytes(b"x"), 8);
    let (h1, h2) = murmur3Sum128(b"x");
    sketch.SubValue(h1, h2, 3);
    assert_eq!(sketch.QueryBytes(b"x"), 5);
    assert!(sketch.MergeCMSketch(&NewCMSketch(4, 128)).is_err());
}

// Go TestCMSketchCoding.
/// 对应 Go TestCMSketchCoding：不含 TopN 的编码往返。
#[test]
fn cmsketch_encoding_without_topn_round_trips() {
    let mut sketch = NewCMSketch(5, 128);
    sketch.InsertBytesByCount(b"encoded", 11);
    let encoded = EncodeCMSketchWithoutTopN(Some(&sketch)).unwrap();
    let decoded = DecodeCMSketch(&encoded).unwrap().unwrap();
    assert!(decoded.Equal(&sketch));
}

#[test]
/// Go CMSketchAndTopNFromProto accepts rows shorter than the first row and
/// leaves the remaining counters initialized to zero.
fn cmsketch_proto_shorter_later_row_keeps_zero_counters() {
    let mut first = tipb::CmSketchRow::new();
    first.set_counters(vec![1, 2]);
    let mut second = tipb::CmSketchRow::new();
    second.set_counters(vec![3]);
    let mut proto = tipb::CmSketch::new();
    proto.set_rows(protobuf::RepeatedField::from_vec(vec![first, second]));

    let (sketch, top_n) = CMSketchAndTopNFromProto(Some(&proto));
    let sketch = sketch.unwrap();
    assert!(top_n.is_none());
    let round_trip = CMSketchToProto(Some(&sketch), None);
    assert_eq!(round_trip.get_rows()[0].get_counters(), &[1, 2]);
    assert_eq!(round_trip.get_rows()[1].get_counters(), &[3, 0]);
    assert_eq!(sketch.TotalCount(), 3);
}

// Go TestCMSketchTopN and TestCMSketchTopNUniqueData.
/// 对应 Go TestCMSketchTopN / TestCMSketchTopNUniqueData：
/// 重复样本应产出 TopN；全唯一样本则不应保留 TopN。
#[test]
fn cmsketch_topn_threshold_and_unique_samples() {
    let repeated = vec![b"x".to_vec(); 20];
    let (_, top_n, _, _) = NewCMSketchAndTopN(5, 128, &repeated, 1, 200);
    assert_eq!(top_n.unwrap().QueryTopN(b"x"), (200, true));
    let unique = (0_u8..20).map(|value| vec![value]).collect::<Vec<_>>();
    let (_, top_n, ndv, _) = NewCMSketchAndTopN(5, 128, &unique, 5, 20);
    assert!(top_n.is_none_or(|top_n| top_n.Num() == 0));
    assert_eq!(ndv, 20);
}

// Go TestCMSketchCodingTopN.
/// 对应 Go TestCMSketchCodingTopN：带 TopN 的 protobuf 编解码。
#[test]
fn cmsketch_topn_proto_coding() {
    let sketch = NewCMSketch(5, 128);
    let mut top_n = NewTopN(1);
    top_n.AppendTopN(vec![1, 2, 3], 9);
    let proto = CMSketchToProto(Some(&sketch), Some(&top_n));
    let (_, decoded) = CMSketchAndTopNFromProto(Some(&proto));
    assert_eq!(decoded.unwrap().QueryTopN(&[1, 2, 3]), (9, true));
}

// Go TestSortTopnMeta.
/// 对应 Go TestSortTopnMeta：先按 Count 降序，Count 相同再按字节升序。
#[test]
fn sort_topn_meta_uses_count_then_bytes() {
    let mut values = vec![
        TopNMeta {
            Encoded: b"b".to_vec(),
            Count: 2,
        },
        TopNMeta {
            Encoded: b"a".to_vec(),
            Count: 2,
        },
        TopNMeta {
            Encoded: b"c".to_vec(),
            Count: 3,
        },
    ];
    SortTopnMeta(&mut values);
    assert_eq!(values[0].Encoded, b"c");
    assert_eq!(values[1].Encoded, b"a");
}

// Go TestTopNScale.
/// 对应 Go TestTopNScale：TotalCount、MinCount、BetweenCount 与 Copy。
#[test]
fn topn_total_min_between_and_copy() {
    let mut top_n = NewTopN(3);
    top_n.AppendTopN(b"a".to_vec(), 3);
    top_n.AppendTopN(b"b".to_vec(), 5);
    top_n.AppendTopN(b"c".to_vec(), 7);
    top_n.Sort();
    assert_eq!(top_n.TotalCount(), 15);
    assert_eq!(top_n.MinCount(), 3);
    assert_eq!(top_n.BetweenCount(b"a", b"c"), 8);
    assert!(top_n.Copy().Equal(&top_n));
}

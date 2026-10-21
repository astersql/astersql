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

// FMSketch 压缩与 protobuf 往返编解码的单元测试。

use crate::*;
use protobuf::Message as _;

/// 插入大量值后应触发 mask 压缩，且 Encode/Decode 后 mask 与哈希集合一致。
#[test]
fn fm_sketch_compacts_and_proto_round_trips() {
    let context = stmtctx::NewStmtCtx();
    let mut sketch = NewFMSketch(8);
    for value in 0..200 {
        sketch
            .InsertValue(&context, types::NewIntDatum(value))
            .unwrap();
    }
    assert!(sketch.mask() > 0);
    assert!(sketch.hash_values().len() <= 8);
    let bytes = EncodeFMSketch(Some(&sketch)).unwrap();
    let decoded = DecodeFMSketch(Some(&bytes)).unwrap().unwrap();
    assert_eq!(decoded.mask(), sketch.mask());
    assert_eq!(decoded.hash_values(), sketch.hash_values());
}

/// Go 的小容量用例要求第三个不同值触发压缩，且保留集合始终不超过上限。
#[test]
fn fm_sketch_deduplicates_and_respects_max_size() {
    let context = stmtctx::NewStmtCtx();
    let mut sketch = NewFMSketch(2);

    sketch.InsertValue(&context, types::NewIntDatum(1)).unwrap();
    sketch.InsertValue(&context, types::NewIntDatum(1)).unwrap();
    assert_eq!(sketch.NDV(), 1);
    assert_eq!(sketch.MemoryUsage(), 110);

    sketch.InsertValue(&context, types::NewIntDatum(2)).unwrap();
    assert_eq!(sketch.hash_values().len(), 2);
    sketch.InsertValue(&context, types::NewIntDatum(4)).unwrap();
    assert!(sketch.hash_values().len() <= 2);
    assert!(sketch.mask() > 0);
}

/// Proto 转换保留 mask/集合并去重；空指针语义与 Go 的 nil 路径一致。
#[test]
fn fm_sketch_proto_and_nil_contracts_match_go() {
    let mut proto = tipb::FmSketch::new();
    proto.set_mask(3);
    proto.set_hashset(vec![4, 8, 4]);

    let sketch = FMSketchFromProto(Some(&proto)).unwrap();
    assert_eq!(sketch.mask(), 3);
    assert_eq!(sketch.hash_values(), vec![4, 8]);
    assert_eq!(sketch.NDV(), 8);
    assert_eq!(sketch.MemoryUsage(), 140);

    let converted = FMSketchToProto(Some(&sketch));
    assert_eq!(converted.get_mask(), 3);
    let mut hashes = converted.get_hashset().to_vec();
    hashes.sort_unstable();
    assert_eq!(hashes, vec![4, 8]);

    assert!(FMSketchFromProto(None).is_none());
    assert_eq!(FMSketchToProto(None).get_mask(), 0);
    assert!(FMSketchToProto(None).get_hashset().is_empty());
    assert!(EncodeFMSketch(None).unwrap().is_empty());
    assert!(DecodeFMSketch(None).unwrap().is_none());
}

/// 合并时采用更稀疏的 mask，并过滤不满足新 mask 的原有哈希值。
#[test]
fn fm_sketch_merge_aligns_to_the_larger_mask() {
    let mut left_proto = tipb::FmSketch::new();
    left_proto.set_mask(0);
    left_proto.set_hashset(vec![1, 4, 8]);
    let left_bytes = left_proto.write_to_bytes().unwrap();
    let mut left = DecodeFMSketch(Some(&left_bytes)).unwrap().unwrap();

    let mut right_proto = tipb::FmSketch::new();
    right_proto.set_mask(3);
    right_proto.set_hashset(vec![4, 12]);
    let right_bytes = right_proto.write_to_bytes().unwrap();
    let right = DecodeFMSketch(Some(&right_bytes)).unwrap().unwrap();

    left.MergeFMSketch(&right);
    assert_eq!(left.mask(), 3);
    assert_eq!(left.hash_values(), vec![4, 8, 12]);
    assert_eq!(left.NDV(), 12);
}

/// Go `SubTestSampledNDV`：合并 sampled sketches 时保留 singleton/repeated 状态，
/// 累加行数并用 GEE 公式估算 NDV。
#[test]
fn sampled_ndv_merges_hash_multiplicity_and_row_counts() {
    fn sampled(singles: Vec<u64>, repeated: Vec<u64>, counts: NdvCounts) -> FMSketch {
        let mut proto = tipb::FmSketch::new();
        proto.set_hashset(singles);
        proto.set_multi_hashset(repeated);
        newSampledFMSketch(&proto, counts)
    }

    let left = sampled(
        vec![1, 3],
        vec![2],
        NdvCounts {
            rows: 50,
            samples: 5,
            nulls: 10,
        },
    );
    let right = sampled(
        vec![1, 4],
        vec![],
        NdvCounts {
            rows: 30,
            samples: 3,
            nulls: 10,
        },
    );

    for (first, second) in [(&left, &right), (&right, &left)] {
        let mut merged = first.Copy();
        merged.MergeFMSketch(second);
        assert_eq!(
            merged.hashset,
            [(1, true), (2, true), (3, false), (4, false)]
                .into_iter()
                .collect::<std::collections::HashMap<u64, bool>>()
        );
        assert_eq!(merged.NDV(), 8);
        assert_eq!(merged.ndvCounts.unwrap().nulls, 20);
        assert_eq!(merged.MemoryUsage(), 224);
    }

    // ANALYZE 先分配普通空 sketch，首个 sampled 响应会把它升级为 sampled。
    let mut root = NewFMSketch(MaxSketchSize);
    root.MergeFMSketch(&left);
    root.MergeFMSketch(&NewFMSketch(MaxSketchSize));
    root.MergeFMSketch(&right);
    assert_eq!(root.ndvCounts.unwrap().rows, 80);
    assert_eq!(root.NDV(), 8);

    let mut empty = sampled(
        vec![],
        vec![],
        NdvCounts {
            rows: 10,
            samples: 0,
            nulls: 0,
        },
    );
    assert_eq!(empty.NDV(), 0);
    empty.MergeFMSketch(&left);
    assert_eq!(empty.ndvCounts.unwrap().rows, 60);
    assert_eq!(empty.ndvCounts.unwrap().samples, 5);
    assert_eq!(empty.NDV(), 8);

    // sample count 字段的“存在性”区分普通与 sampled sketch，值为 0 也仍是 sampled。
    for sampled_input in [false, true] {
        let mut proto = tipb::RowSampleCollector::new();
        proto.set_count(1);
        proto.set_null_counts(vec![0]);
        proto.set_total_size(vec![0]);
        proto.set_fm_sketch(protobuf::RepeatedField::from_vec(vec![
            tipb::FmSketch::new(),
        ]));
        if sampled_input {
            proto.set_ndv_sample_count(0);
        }
        let restored = baseCollector::FromProto(&proto);
        assert_eq!(restored.FMSketches[0].ndvCounts.is_some(), sampled_input);
    }

    // 无样本行或没有非 NULL 行时 NDV 为 0。
    assert_eq!(
        sampled(
            vec![],
            vec![],
            NdvCounts {
                rows: 200,
                samples: 0,
                nulls: 0
            }
        )
        .NDV(),
        0
    );
    assert_eq!(
        sampled(
            vec![],
            vec![],
            NdvCounts {
                rows: 10,
                samples: 2,
                nulls: 10
            }
        )
        .NDV(),
        0
    );

    // schema 保证唯一时，BuildHistAndTopN 使用行数而不是 sampled sketch 估算。
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(15, MaxSketchSize);
    collector.FMSketch = sampled(
        (1..=14).collect(),
        vec![],
        NdvCounts {
            rows: 3_000,
            samples: 15,
            nulls: 0,
        },
    );
    for value in 0..15 {
        collector
            .Collect(&context, types::NewIntDatum(value))
            .unwrap();
    }
    collector.Count = 3_000;
    collector.Unique = true;
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let (histogram, _) =
        BuildHistAndTopN(&context, 256, 0, 1, &mut collector, &field_type, true).unwrap();
    assert_eq!(histogram.NDV, 3_000);
}

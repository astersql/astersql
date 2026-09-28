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
    assert_eq!(sketch.MemoryUsage(), 24);

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
    assert_eq!(sketch.MemoryUsage(), 32);

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

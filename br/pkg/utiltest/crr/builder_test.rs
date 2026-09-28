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

//! Go-equivalent tests for `br/pkg/utiltest/crr/builder_test.go`.
//! 假集群 region 布局构建器：store ID 范围、轮询与按 split key 划分。
//! 供 CRR 相关单测构造可预测的 region/store 拓扑。
//! 键边界字节与 Go 测试向量一致（如 k01）。

use crate::builder::{
    AddRegionsBySplitKeys, AddRoundRobinRegions, BuildRegionLayout, StoreIDRange,
};
use crate::types::RegionBoundary;

/// Corresponds to Go `TestStoreIDRange`.
/// 闭区间 [start,end]；count<=0 时 Go 返回 nil，Rust 为空 Vec。
#[test]
fn test_store_i_d_range() {
    assert_eq!(StoreIDRange(1, 4), vec![1_u64, 2, 3, 4]);
    // Go returns nil for count <= 0; Rust returns an empty Vec.
    assert!(StoreIDRange(1, 0).is_empty());
}

/// Corresponds to Go `TestAddRoundRobinRegions`.
/// 5 个 region 在 3 store 上轮询；首尾键边界为空表示无界。
#[test]
fn test_add_round_robin_regions() {
    let stores = StoreIDRange(1, 3);
    let boundaries = BuildRegionLayout(vec![AddRoundRobinRegions(5, stores)]).unwrap();
    assert_eq!(boundaries.len(), 5);

    assert!(boundaries[0].StartKey.is_empty());
    assert_eq!(boundaries[0].EndKey, b"k01");
    assert_eq!(boundaries[0].StoreID, 1);

    // 最后一个 region end 为空，store 按轮询落到 2。
    assert_eq!(boundaries[4].StartKey, b"k04");
    assert!(boundaries[4].EndKey.is_empty());
    assert_eq!(boundaries[4].StoreID, 2);
}

/// Corresponds to Go `TestAddRegionsBySplitKeys`.
/// 两个 split key 切出三段，store 列表循环分配。
#[test]
fn test_add_regions_by_split_keys() {
    let boundaries = BuildRegionLayout(vec![AddRegionsBySplitKeys(
        vec!["a".to_string(), "d".to_string()],
        vec![10, 11],
    )])
    .unwrap();
    assert_eq!(boundaries.len(), 3);

    assert_eq!(boundaries[0].StartKey, b"");
    assert_eq!(boundaries[0].EndKey, b"a");
    assert_eq!(boundaries[0].StoreID, 10);

    assert_eq!(boundaries[1].StartKey, b"a");
    assert_eq!(boundaries[1].EndKey, b"d");
    assert_eq!(boundaries[1].StoreID, 11);

    // 末段回到 stores[0]=10。
    assert_eq!(boundaries[2].StartKey, b"d");
    assert!(boundaries[2].EndKey.is_empty());
    assert_eq!(boundaries[2].StoreID, 10);
}

#[test]
fn test_store_id_range_wraps_like_go_uint64() {
    assert_eq!(StoreIDRange(u64::MAX, 2), vec![u64::MAX, 0]);
}

#[test]
fn test_append_after_binary_boundary_preserves_go_string_bytes() {
    let binary_prefix = Box::new(|_| {
        Ok(vec![RegionBoundary {
            StartKey: Vec::new(),
            EndKey: vec![0xff],
            StoreID: 1,
        }])
    });

    let boundaries = BuildRegionLayout(vec![
        binary_prefix,
        AddRegionsBySplitKeys(Vec::new(), vec![2]),
    ])
    .unwrap();

    assert_eq!(boundaries[1].StartKey, vec![0xff]);
    assert!(boundaries[1].EndKey.is_empty());
}

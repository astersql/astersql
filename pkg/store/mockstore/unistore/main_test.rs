// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// unistore 包测试入口配置（对应 Go `TestMain`）。
//
// unistore 是 TiDB 单测用的嵌入式 mock TiKV：在进程内提供 Region、MVCC、
// Raft 写入等存储能力，无需真实集群。

/// `Cluster::split_raw` must preserve Go's raw-key boundary contract by
/// encoding the user key before handing it to the Region manager.
#[test]
fn test_cluster_split_raw_encodes_user_key() {
    let manager = std::sync::Arc::new(crate::tikv::mock_region::MockRegionManager::new(1, 0));
    let cluster = crate::Cluster::new(std::sync::Arc::clone(&manager));
    let (_, peer_id, region_id) =
        crate::BootstrapWithSingleStore(&cluster).expect("bootstrap mock cluster");
    let split_key = b"raw-split".to_vec();
    let new_region_id = manager.alloc_id();

    let right = cluster
        .split_raw(
            region_id,
            new_region_id,
            split_key.clone(),
            &[peer_id],
            peer_id,
        )
        .expect("split raw key");

    assert_eq!(right.start_key, crate::encode_bytes(&split_key));

    // `Cluster::split` has the same public raw-key contract, but must encode
    // exactly once instead of routing an already encoded key through
    // `split_raw`.
    let manager = std::sync::Arc::new(crate::tikv::mock_region::MockRegionManager::new(1, 0));
    let cluster = crate::Cluster::new(std::sync::Arc::clone(&manager));
    let (_, peer_id, region_id) =
        crate::BootstrapWithSingleStore(&cluster).expect("bootstrap mock cluster");
    let split_key = b"normal-split".to_vec();
    let new_region_id = manager.alloc_id();
    let right = cluster
        .split(region_id, new_region_id, &split_key, &[peer_id], peer_id)
        .expect("split user key");
    assert_eq!(right.start_key, crate::encode_bytes(&split_key));
}

#[test]
fn common_test_environment_is_initialized() {
    assert!(crate::TEST_ENVIRONMENT_INITIALIZED.load(std::sync::atomic::Ordering::Acquire));
}

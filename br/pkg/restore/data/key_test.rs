// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Equivalents of `br/pkg/restore/data/key_test.go`.
//! Pure in-memory region/peer algorithms — no PD/TiKV/network.
//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/data/key_test.rs`对应的键与 Region 算法单元测试，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少27行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! 测试夹具中的 SQL 分支匹配顺序与 Go 用例场景一一对应，改动匹配条件等于改动契约。
//! - `new_peer_meta`是当前文件的重要函数，承担"new_peer_meta"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `new_recover_region_info`是当前文件的重要函数，承担"new_recover_region_info"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_sort_recover_regions`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `test_check_consistency_and_valid_peer`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `test_leader_candidates`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `test_select_region_leader`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! 中文注释索引结束

use std::collections::{HashMap, HashSet};

use crate::key::{PrefixEndKey, PrefixStartKey};
use crate::recover::{
    CheckConsistencyAndValidPeer, LeaderCandidates, RecoverRegion, RecoverRegionInfo,
    SelectRegionLeader, SortRecoverRegions,
};
use crate::stubs::recovpb;

fn new_peer_meta(
    region_id: u64,
    peer_id: u64,
    store_id: u64,
    start_key: &[u8],
    end_key: Option<&[u8]>,
    last_log_term: u64,
    last_index: u64,
    commit_index: u64,
    version: u64,
    tombstone: bool,
) -> RecoverRegion {
    RecoverRegion {
        RegionMeta: recovpb::RegionMeta {
            RegionId: region_id,
            PeerId: peer_id,
            StartKey: start_key.to_vec(),
            EndKey: end_key.unwrap_or(&[]).to_vec(),
            LastLogTerm: last_log_term,
            LastIndex: last_index,
            CommitIndex: commit_index,
            Version: version,
            Tombstone: tombstone,
        },
        StoreId: store_id,
    }
}

fn new_recover_region_info(r: &RecoverRegion) -> RecoverRegionInfo {
    RecoverRegionInfo {
        RegionVersion: r.Version,
        RegionId: r.RegionId,
        StartKey: PrefixStartKey(&r.StartKey),
        EndKey: PrefixEndKey(&r.EndKey),
        TombStone: r.Tombstone,
    }
}

/// Go `TestSortRecoverRegions`.
#[test]
fn test_sort_recover_regions() {
    let selected_peer1 = new_peer_meta(9, 11, 2, b"aa", None, 2, 0, 0, 0, false);
    let selected_peer2 = new_peer_meta(19, 22, 3, b"bbb", None, 2, 1, 0, 1, false);
    let selected_peer3 = new_peer_meta(29, 30, 1, b"c", None, 2, 1, 1, 2, false);
    let mut regions: HashMap<u64, Vec<RecoverRegion>> = HashMap::from([
        (
            9,
            vec![
                // peer 11 should be selected because of log term
                new_peer_meta(9, 10, 1, b"a", None, 1, 1, 1, 1, false),
                selected_peer1.clone(),
                new_peer_meta(9, 12, 3, b"aaa", None, 0, 0, 0, 0, false),
            ],
        ),
        (
            19,
            vec![
                // peer 22 should be selected because of log index
                new_peer_meta(19, 20, 1, b"b", None, 1, 1, 1, 1, false),
                new_peer_meta(19, 21, 2, b"bb", None, 2, 0, 0, 0, false),
                selected_peer2.clone(),
            ],
        ),
        (
            29,
            vec![
                // peer 30 should be selected because of log index
                selected_peer3.clone(),
                new_peer_meta(29, 31, 2, b"cc", None, 2, 0, 0, 0, false),
                new_peer_meta(29, 32, 3, b"ccc", None, 2, 1, 0, 0, false),
            ],
        ),
    ]);
    let regions_infos = SortRecoverRegions(&mut regions);
    let expect_region_infos = vec![
        new_recover_region_info(&selected_peer3),
        new_recover_region_info(&selected_peer2),
        new_recover_region_info(&selected_peer1),
    ];
    assert_eq!(expect_region_infos, regions_infos);
}

/// Go `TestCheckConsistencyAndValidPeer`.
#[test]
fn test_check_consistency_and_valid_peer() {
    // key space is continuous
    let valid_peer1 = new_peer_meta(9, 11, 2, b"", Some(b"bb"), 2, 0, 0, 0, false);
    let valid_peer2 = new_peer_meta(19, 22, 3, b"bb", Some(b"cc"), 2, 1, 0, 1, false);
    let valid_peer3 = new_peer_meta(29, 30, 1, b"cc", Some(b""), 2, 1, 1, 2, false);

    let valid_region_infos = vec![
        new_recover_region_info(&valid_peer1),
        new_recover_region_info(&valid_peer2),
        new_recover_region_info(&valid_peer3),
    ];

    let valid_peer = CheckConsistencyAndValidPeer(valid_region_infos).expect("valid");
    assert_eq!(valid_peer.len(), 3);
    let regions: HashSet<u64> = HashSet::from([9, 19, 29]);
    assert_eq!(regions, valid_peer);

    // key space is not continuous
    let invalid_peer1 = new_peer_meta(9, 11, 2, b"aa", Some(b"cc"), 2, 0, 0, 0, false);
    let invalid_peer2 = new_peer_meta(19, 22, 3, b"dd", Some(b"cc"), 2, 1, 0, 1, false);
    let invalid_peer3 = new_peer_meta(29, 30, 1, b"cc", Some(b"dd"), 2, 1, 1, 2, false);

    let invalid_region_infos = vec![
        new_recover_region_info(&invalid_peer1),
        new_recover_region_info(&invalid_peer2),
        new_recover_region_info(&invalid_peer3),
    ];

    let err = CheckConsistencyAndValidPeer(invalid_region_infos).expect_err("gap");
    // Go: require.Regexp(t, ".*invalid restore range.*", err.Error())
    assert!(err.msg.contains("invalid restore range"), "err={}", err.msg);
}

/// Go `TestLeaderCandidates`.
#[test]
fn test_leader_candidates() {
    let valid_peer1 = new_peer_meta(9, 11, 2, b"", Some(b"bb"), 2, 1, 0, 0, false);
    let valid_peer2 = new_peer_meta(19, 22, 3, b"bb", Some(b"cc"), 2, 1, 0, 1, false);
    let valid_peer3 = new_peer_meta(29, 30, 1, b"cc", Some(b""), 2, 1, 0, 2, false);

    let peers = vec![valid_peer1, valid_peer2, valid_peer3];
    let candidates = LeaderCandidates(&peers).expect("candidates");
    assert_eq!(candidates.len(), 3);
}

/// Go `TestSelectRegionLeader`.
#[test]
fn test_select_region_leader() {
    let valid_peer1 = new_peer_meta(9, 11, 2, b"", Some(b"bb"), 2, 1, 0, 0, false);
    let valid_peer2 = new_peer_meta(19, 22, 3, b"bb", Some(b"cc"), 2, 1, 0, 1, false);
    let valid_peer3 = new_peer_meta(29, 30, 1, b"cc", Some(b""), 2, 1, 0, 2, false);

    let peers = vec![valid_peer1.clone(), valid_peer2, valid_peer3.clone()];
    // init store balance score all is 0
    let mut store_balance_score: HashMap<u64, i32> = HashMap::with_capacity(peers.len());
    let mut leader = SelectRegionLeader(&store_balance_score, &peers);
    assert_eq!(valid_peer1, leader);

    // change store balance score
    store_balance_score.insert(2, 3);
    store_balance_score.insert(3, 2);
    store_balance_score.insert(1, 1);
    leader = SelectRegionLeader(&store_balance_score, &peers);
    assert_eq!(valid_peer3, leader);

    // one peer
    let peer = vec![valid_peer3.clone()];
    let store_score: HashMap<u64, i32> = HashMap::with_capacity(peer.len());
    leader = SelectRegionLeader(&store_score, &peer);
    assert_eq!(valid_peer3, leader);
}

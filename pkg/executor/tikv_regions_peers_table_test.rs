// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `INFORMATION_SCHEMA.TIKV_REGION_PEERS` 行打包单元测试。
//
// Region 是 TiKV 的数据分片；Peer 是 Region 在某 Store 上的副本。
// 本测试校验 leader / PENDING / DOWN 等 Peer 状态列的分类与写入。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::memtable_reader::{
    HistoryHotRegion, HistoryHotRegions, HistoryHotRegionsRequest, MemTableError, MemTableResult,
    cancellation, configItem, datum, datumRow, downPeerStat, dummyCloser, logSearchRequest,
    logStream, memTableRuntime, peerInfo, regionInfo, runtimeStats, serverInfo, serverInfoType,
    tableMapping, tikvRegionPeersExtractor, tikvRegionPeersRetriever,
};

/// 内存表运行时桩：按 Go 测试的 PD HTTP fixture 返回 Region 拓扑。
struct RegionRuntime {
    regions: BTreeMap<u64, regionInfo>,
}

impl RegionRuntime {
    fn go_fixture() -> Self {
        let regions = (1..=3)
            .map(|region_id| {
                let peers = (1..=3)
                    .map(|store_id| peerInfo {
                        id: region_id * 10 + store_id,
                        storeID: store_id,
                        isLearner: false,
                    })
                    .collect::<Vec<_>>();
                let leader = peers[(region_id - 1) as usize].clone();
                (
                    region_id as u64,
                    regionInfo {
                        id: region_id,
                        peers,
                        pendingPeers: Vec::new(),
                        downPeers: Vec::new(),
                        leader: Some(leader),
                    },
                )
            })
            .collect();
        Self { regions }
    }
}

impl memTableRuntime for RegionRuntime {
    fn executor_open(&self) -> MemTableResult {
        unimplemented!()
    }
    fn probe_transaction(&self) -> MemTableResult<bool> {
        unimplemented!()
    }
    fn activate_transaction(&self) -> MemTableResult {
        unimplemented!()
    }
    fn internal_http_schema(&self) -> String {
        unimplemented!()
    }
    fn has_config_privilege(&self) -> bool {
        unimplemented!()
    }
    fn has_process_privilege(&self) -> bool {
        unimplemented!()
    }
    fn cluster_servers(&self) -> MemTableResult<Vec<serverInfo>> {
        unimplemented!()
    }
    fn pd_servers(&self) -> MemTableResult<Vec<serverInfo>> {
        unimplemented!()
    }
    fn fetch_cluster_config(&self, _: &serverInfo, _: &str) -> MemTableResult<Vec<configItem>> {
        unimplemented!()
    }
    fn fetch_server_info(
        &self,
        _: &serverInfo,
        _: serverInfoType,
    ) -> MemTableResult<Vec<datumRow>> {
        unimplemented!()
    }
    fn new_log_cancellation(&self) -> Arc<dyn cancellation> {
        unimplemented!()
    }
    fn open_log_stream(
        &self,
        _: &serverInfo,
        _: &str,
        _: &logSearchRequest,
        _: Arc<dyn cancellation>,
    ) -> MemTableResult<Box<dyn logStream>> {
        unimplemented!()
    }
    fn fetch_hot_regions(
        &self,
        _: &serverInfo,
        _: &HistoryHotRegionsRequest,
    ) -> MemTableResult<HistoryHotRegions> {
        unimplemented!()
    }
    fn ensure_tikv_storage(&self) -> MemTableResult {
        Ok(())
    }
    fn hot_region_table_mappings(&self, _: &HistoryHotRegion) -> MemTableResult<Vec<tableMapping>> {
        unimplemented!()
    }
    fn format_timestamp(&self, _: i64) -> MemTableResult<String> {
        unimplemented!()
    }
    fn all_regions(&self) -> MemTableResult<Vec<regionInfo>> {
        Ok(self.regions.values().cloned().collect())
    }
    fn regions_by_store(&self, store_id: u64) -> MemTableResult<Vec<regionInfo>> {
        Ok(self
            .regions
            .values()
            .filter(|region| {
                region
                    .peers
                    .iter()
                    .any(|peer| peer.storeID == store_id as i64)
            })
            .cloned()
            .collect())
    }
    fn region_by_id(&self, region_id: u64) -> MemTableResult<Option<regionInfo>> {
        Ok(self.regions.get(&region_id).cloned())
    }
    fn append_warning(&self, _: MemTableError) {}
    fn register_runtime_stats(&self, _: runtimeStats) {}
}

fn go_fixture_retriever(store_ids: Vec<u64>, region_ids: Vec<u64>) -> tikvRegionPeersRetriever {
    tikvRegionPeersRetriever {
        dummyCloser,
        extractor: tikvRegionPeersExtractor {
            skipRequest: false,
            storeIDs: store_ids,
            regionIDs: region_ids,
        },
        retrieved: false,
        runtime: Arc::new(RegionRuntime::go_fixture()),
    }
}

fn row_values(row: &[datum]) -> Vec<i64> {
    row.iter()
        .take(5)
        .map(|value| match value {
            datum::Int(value) => *value,
            other => panic!("expected integer TiKV peer column, got {other:?}"),
        })
        .collect()
}

#[test]
fn tikv_region_peers_matches_go_query_cases() {
    struct Case {
        store_ids: Vec<u64>,
        region_ids: Vec<u64>,
        is_leader: Option<i64>,
        expected: Vec<Vec<i64>>,
    }

    let full = vec![
        vec![1, 11, 1, 0, 1],
        vec![1, 12, 2, 0, 0],
        vec![1, 13, 3, 0, 0],
        vec![2, 21, 1, 0, 0],
        vec![2, 22, 2, 0, 1],
        vec![2, 23, 3, 0, 0],
        vec![3, 31, 1, 0, 0],
        vec![3, 32, 2, 0, 0],
        vec![3, 33, 3, 0, 1],
    ];
    let cases = vec![
        Case {
            store_ids: vec![1, 2, 3],
            region_ids: vec![1, 2, 3],
            is_leader: None,
            expected: full,
        },
        Case {
            store_ids: vec![1, 2],
            region_ids: vec![1],
            is_leader: None,
            expected: vec![vec![1, 11, 1, 0, 1], vec![1, 12, 2, 0, 0]],
        },
        Case {
            store_ids: vec![1, 2],
            region_ids: vec![1],
            is_leader: Some(1),
            expected: vec![vec![1, 11, 1, 0, 1]],
        },
        Case {
            store_ids: vec![1, 2],
            region_ids: vec![1],
            is_leader: Some(0),
            expected: vec![vec![1, 12, 2, 0, 0]],
        },
        Case {
            store_ids: vec![1],
            region_ids: vec![1],
            is_leader: Some(0),
            expected: Vec::new(),
        },
    ];

    for case in cases {
        let mut retriever = go_fixture_retriever(case.store_ids, case.region_ids);
        let actual = retriever
            .retrieve()
            .unwrap()
            .into_iter()
            .filter(|row| {
                case.is_leader
                    .is_none_or(|wanted| row[4] == datum::Int(wanted))
            })
            .map(|row| row_values(&row))
            .collect::<Vec<_>>();
        assert_eq!(actual, case.expected);
        assert!(retriever.retrieve().unwrap().is_empty());
    }

    let mut retriever = go_fixture_retriever(Vec::new(), Vec::new());
    assert_eq!(retriever.retrieve().unwrap().len(), 9);
}

#[test]
/// 同一 Region 内：leader 为 NORMAL、pending 为 PENDING、宕机 learner 为 DOWN 并带 downSeconds。
fn tikv_region_peer_rows_classify_leader_pending_and_down_peers() {
    // 构造 leader / pending / down 三类 Peer，down 为 learner 且宕机 9 秒。
    let leader = peerInfo {
        id: 11,
        storeID: 1,
        isLearner: false,
    };
    let pending = peerInfo {
        id: 12,
        storeID: 2,
        isLearner: false,
    };
    let down = peerInfo {
        id: 13,
        storeID: 3,
        isLearner: true,
    };
    let retriever = tikvRegionPeersRetriever {
        dummyCloser,
        extractor: tikvRegionPeersExtractor::default(),
        retrieved: false,
        runtime: Arc::new(RegionRuntime::go_fixture()),
    };
    // 打包行后核对：是否 leader、状态字符串、learner 标记与宕机时长。
    let rows = retriever
        .packTiKVRegionPeersRows(
            vec![regionInfo {
                id: 7,
                peers: vec![leader.clone(), pending.clone(), down.clone()],
                pendingPeers: vec![pending],
                downPeers: vec![downPeerStat {
                    peer: down,
                    downSeconds: 9,
                }],
                leader: Some(leader),
            }],
            &BTreeSet::new(),
        )
        .unwrap();
    assert_eq!(rows[0][4], datum::Int(1));
    assert_eq!(rows[0][5], datum::String("NORMAL".into()));
    assert_eq!(rows[1][5], datum::String("PENDING".into()));
    assert_eq!(rows[2][3], datum::Int(1));
    assert_eq!(rows[2][5], datum::String("DOWN".into()));
    assert_eq!(rows[2][6], datum::Int(9));
}

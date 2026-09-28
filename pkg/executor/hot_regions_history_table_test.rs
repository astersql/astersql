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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::memtable_reader::{
    HistoryHotRegion, HistoryHotRegions, HistoryHotRegionsRequest, MemTableError, MemTableResult,
    cancellation, configItem, datum, datumRow, dummyCloser, hotRegionsHistoryRetriver,
    hotRegionsHistoryTableExtractor, hotRegionsResponseHeap, hotRegionsResult, logSearchRequest,
    logStream, memTableRuntime, regionInfo, runtimeStats, serverInfo, serverInfoType, tableMapping,
};

fn response(addr: &str, regions: &[(i64, i64, u64)]) -> hotRegionsResult {
    hotRegionsResult {
        addr: addr.to_owned(),
        messages: HistoryHotRegions {
            historyHotRegion: regions
                .iter()
                .map(|&(update_time, hot_degree, region_id)| HistoryHotRegion {
                    updateTime: update_time,
                    hotDegree: hot_degree,
                    regionID: region_id,
                    ..HistoryHotRegion::default()
                })
                .collect(),
        },
        error: None,
    }
}

#[derive(Default)]
struct HotRegionRuntime {
    process_privilege: bool,
    pd_servers: Vec<serverInfo>,
    responses: BTreeMap<(String, String), MemTableResult<HistoryHotRegions>>,
    requests: Mutex<Vec<(String, HistoryHotRegionsRequest)>>,
    warnings: Mutex<Vec<MemTableError>>,
}

impl memTableRuntime for HotRegionRuntime {
    fn executor_open(&self) -> MemTableResult {
        panic!("unexpected executor_open call")
    }
    fn probe_transaction(&self) -> MemTableResult<bool> {
        panic!("unexpected probe_transaction call")
    }
    fn activate_transaction(&self) -> MemTableResult {
        panic!("unexpected activate_transaction call")
    }
    fn internal_http_schema(&self) -> String {
        panic!("unexpected internal_http_schema call")
    }
    fn has_config_privilege(&self) -> bool {
        panic!("unexpected has_config_privilege call")
    }
    fn has_process_privilege(&self) -> bool {
        self.process_privilege
    }
    fn cluster_servers(&self) -> MemTableResult<Vec<serverInfo>> {
        panic!("unexpected cluster_servers call")
    }
    fn pd_servers(&self) -> MemTableResult<Vec<serverInfo>> {
        Ok(self.pd_servers.clone())
    }
    fn fetch_cluster_config(&self, _: &serverInfo, _: &str) -> MemTableResult<Vec<configItem>> {
        panic!("unexpected fetch_cluster_config call")
    }
    fn fetch_server_info(
        &self,
        _: &serverInfo,
        _: serverInfoType,
    ) -> MemTableResult<Vec<datumRow>> {
        panic!("unexpected fetch_server_info call")
    }
    fn new_log_cancellation(&self) -> Arc<dyn cancellation> {
        panic!("unexpected new_log_cancellation call")
    }
    fn open_log_stream(
        &self,
        _: &serverInfo,
        _: &str,
        _: &logSearchRequest,
        _: Arc<dyn cancellation>,
    ) -> MemTableResult<Box<dyn logStream>> {
        panic!("unexpected open_log_stream call")
    }
    fn fetch_hot_regions(
        &self,
        server: &serverInfo,
        request: &HistoryHotRegionsRequest,
    ) -> MemTableResult<HistoryHotRegions> {
        self.requests
            .lock()
            .unwrap()
            .push((server.statusAddr.clone(), request.clone()));
        let hot_type = request.hotRegionTypes.first().cloned().unwrap_or_default();
        self.responses
            .get(&(server.statusAddr.clone(), hot_type))
            .cloned()
            .unwrap_or_else(|| Ok(HistoryHotRegions::default()))
    }
    fn ensure_tikv_storage(&self) -> MemTableResult {
        Ok(())
    }
    fn hot_region_table_mappings(
        &self,
        region: &HistoryHotRegion,
    ) -> MemTableResult<Vec<tableMapping>> {
        if region.regionID == 1313 {
            return Ok(Vec::new());
        }
        Ok(vec![tableMapping {
            databaseName: "mysql".into(),
            tableName: if region.regionID == 1 {
                "tables_priv"
            } else {
                "stats_meta"
            }
            .into(),
            tableID: region.regionID as i64,
            indexName: (region.regionID == 2).then(|| "idx_ver".into()),
            indexID: (region.regionID == 2).then_some(1),
        }])
    }
    fn format_timestamp(&self, unix_millis: i64) -> MemTableResult<String> {
        Ok(format!("ts-{unix_millis}"))
    }
    fn all_regions(&self) -> MemTableResult<Vec<regionInfo>> {
        panic!("unexpected all_regions call")
    }
    fn regions_by_store(&self, _: u64) -> MemTableResult<Vec<regionInfo>> {
        panic!("unexpected regions_by_store call")
    }
    fn region_by_id(&self, _: u64) -> MemTableResult<Option<regionInfo>> {
        panic!("unexpected region_by_id call")
    }
    fn append_warning(&self, warning: MemTableError) {
        self.warnings.lock().unwrap().push(warning);
    }
    fn register_runtime_stats(&self, _: runtimeStats) {}
}

fn retriever(
    runtime: Arc<dyn memTableRuntime>,
    extractor: hotRegionsHistoryTableExtractor,
) -> hotRegionsHistoryRetriver {
    hotRegionsHistoryRetriver {
        dummyCloser,
        isDrained: false,
        retrieving: false,
        heap: hotRegionsResponseHeap::default(),
        extractor,
        runtime,
    }
}

#[test]
fn hot_region_response_heap_orders_time_then_hot_degree() {
    let mut heap = hotRegionsResponseHeap::default();
    heap.Push(response("later", &[(20, 1, 1)]));
    heap.Push(response("hotter", &[(10, 3, 1)]));
    heap.Push(response("cooler", &[(10, 1, 1)]));
    assert_eq!(heap.Len(), 3);
    assert_eq!(heap.Pop().unwrap().addr, "cooler");
    assert_eq!(heap.Pop().unwrap().addr, "hotter");
    assert_eq!(heap.Pop().unwrap().addr, "later");
    assert!(heap.Pop().is_none());
}

#[test]
fn hot_regions_history_requires_process_privilege_and_bounded_time_range() {
    let base = hotRegionsHistoryTableExtractor {
        startTime: 10,
        endTime: 20,
        hotRegionTypes: BTreeSet::from(["READ".into()]),
        ..Default::default()
    };
    assert_eq!(
        retriever(Arc::new(HotRegionRuntime::default()), base.clone())
            .initialize()
            .unwrap_err()
            .0,
        "PROCESS privilege required"
    );
    let runtime = Arc::new(HotRegionRuntime {
        process_privilege: true,
        ..Default::default()
    });
    let mut missing_start = base.clone();
    missing_start.startTime = 0;
    assert_eq!(
        retriever(runtime.clone(), missing_start)
            .initialize()
            .unwrap_err()
            .0,
        "denied to scan hot regions, please specified the start time, such as `update_time > '2020-01-01 00:00:00'`"
    );
    let mut missing_end = base;
    missing_end.endTime = 0;
    assert_eq!(
        retriever(runtime, missing_end).initialize().unwrap_err().0,
        "denied to scan hot regions, please specified the end time, such as `update_time < '2020-01-01 00:00:00'`"
    );
}

#[test]
fn hot_regions_history_fans_out_filters_merges_rows_and_warns_like_go() {
    let servers = vec![
        serverInfo {
            serverType: "pd".into(),
            address: "pd-1".into(),
            statusAddr: "pd-1".into(),
        },
        serverInfo {
            serverType: "pd".into(),
            address: "pd-2".into(),
            statusAddr: "pd-2".into(),
        },
    ];
    let mut responses = BTreeMap::new();
    responses.insert(
        ("pd-1".into(), "READ".into()),
        Ok(response("", &[(10, 1, 1), (30, 1, 1313)]).messages),
    );
    responses.insert(
        ("pd-1".into(), "WRITE".into()),
        Err(MemTableError("pd unavailable".into())),
    );
    responses.insert(
        ("pd-2".into(), "READ".into()),
        Ok(response("", &[(20, 1, 2)]).messages),
    );
    let runtime = Arc::new(HotRegionRuntime {
        process_privilege: true,
        pd_servers: servers,
        responses,
        ..Default::default()
    });
    let extractor = hotRegionsHistoryTableExtractor {
        startTime: 100,
        endTime: 200,
        regionIDs: vec![1, 2],
        storeIDs: vec![3],
        peerIDs: vec![4],
        isLearners: vec![false],
        isLeaders: vec![true],
        hotRegionTypes: BTreeSet::from(["READ".into(), "WRITE".into()]),
        ..Default::default()
    };
    let mut retriever = retriever(runtime.clone(), extractor);
    let rows = retriever.retrieve().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], datum::Timestamp("ts-10".into()));
    assert_eq!(rows[0][1], datum::String("MYSQL".into()));
    assert_eq!(rows[0][2], datum::String("TABLES_PRIV".into()));
    assert_eq!(rows[0][4], datum::Null);
    assert_eq!(rows[1][0], datum::Timestamp("ts-20".into()));
    assert_eq!(rows[1][2], datum::String("STATS_META".into()));
    assert_eq!(rows[1][4], datum::String("IDX_VER".into()));
    assert_eq!(rows[1][5], datum::Int(1));
    assert!(retriever.isDrained);
    assert!(retriever.retrieve().unwrap().is_empty());

    let requests = runtime.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for (_, request) in requests.iter() {
        assert_eq!(request.startTime, 100);
        assert_eq!(request.endTime, 200);
        assert_eq!(request.regionIDs, vec![1, 2]);
        assert_eq!(request.storeIDs, vec![3]);
        assert_eq!(request.peerIDs, vec![4]);
        assert_eq!(request.isLearners, vec![false]);
        assert_eq!(request.isLeaders, vec![true]);
        assert_eq!(request.hotRegionTypes.len(), 1);
    }
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        &[MemTableError("pd unavailable".into())]
    );
}

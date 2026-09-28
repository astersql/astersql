// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::batch_coprocessor::BatchTaskSource;
use crate::batch_request_sender::{
    BatchResult, KeyRanges, RegionInfo, RegionVerId, RpcContext, Store,
};
use crate::mpp::{
    DispatchMppTaskRequest, MppCancelRequest, MppClient, MppConnectionRequest, MppDispatchResponse,
    MppDispatchWireRequest, MppQueryId, MppStream, MppTransport,
};

#[derive(Default)]
struct UnusedSource;

impl BatchTaskSource for UnusedSource {
    fn split_key_ranges(&self, _: &KeyRanges, _: i64) -> BatchResult<Vec<RegionInfo>> {
        unreachable!()
    }

    fn fetch_topology(&self) -> BatchResult<Vec<String>> {
        unreachable!()
    }

    fn compute_stores(&self) -> BatchResult<Vec<Store>> {
        unreachable!()
    }

    fn is_store_alive(&self, _: &str, _: Duration) -> bool {
        unreachable!()
    }

    fn rpc_context(&self, _: RegionVerId, _: bool) -> BatchResult<Option<RpcContext>> {
        unreachable!()
    }

    fn all_valid_store_ids(&self, _: RegionVerId, _: u64) -> Vec<u64> {
        unreachable!()
    }

    fn all_tiflash_stores(&self) -> Vec<Store> {
        unreachable!()
    }

    fn invalidate_region(&self, _: RegionVerId) {
        unreachable!()
    }
}

#[derive(Default)]
struct RecordingTransport {
    cancelled: Mutex<Vec<MppCancelRequest>>,
}

impl MppTransport for RecordingTransport {
    fn dispatch(
        &self,
        _: &str,
        _: &MppDispatchWireRequest,
        _: Duration,
    ) -> BatchResult<MppDispatchResponse> {
        unreachable!()
    }

    fn cancel(&self, _: &str, request: &MppCancelRequest, _: Duration) -> BatchResult<()> {
        self.cancelled.lock().unwrap().push(request.clone());
        Ok(())
    }

    fn establish(&self, _: &str, _: &MppConnectionRequest, _: Duration) -> BatchResult<MppStream> {
        unreachable!()
    }

    fn check_visibility(&self, _: u64) -> BatchResult<()> {
        unreachable!()
    }

    fn all_stores(&self) -> BatchResult<Vec<Store>> {
        unreachable!()
    }

    fn invalidate_region(&self, _: RegionVerId) {
        unreachable!()
    }

    fn invalidate_compute_stores(&self) {
        unreachable!()
    }
}

fn request(server_id: u64) -> DispatchMppTaskRequest {
    DispatchMppTaskRequest {
        start_ts: 11,
        query_id: MppQueryId {
            query_ts: 12,
            local_query_id: 13,
            server_id,
        },
        id: 14,
        gather_id: 15,
        address: "store".to_owned(),
        coordinator_address: "coordinator".to_owned(),
        report_execution_summary: true,
        mpp_version: 16,
        resource_group_name: "resource-group".to_owned(),
        connection_id: 17,
        connection_alias: "alias".to_owned(),
        sql_digest: vec![18],
        plan_digest: vec![19],
        ..DispatchMppTaskRequest::default()
    }
}

#[test]
fn repeated_cancel_broadcasts_and_wire_meta_matches_go() {
    let transport = Arc::new(RecordingTransport::default());
    let client = MppClient::new(Arc::new(UnusedSource), transport.clone(), false, false);
    let addresses = HashSet::from(["store".to_owned()]);

    client.cancel_mpp_tasks(&addresses, &[request(21)]);
    client.cancel_mpp_tasks(&addresses, &[request(21)]);

    let requests = transport.cancelled.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(request.meta.start_ts, 11);
        assert_eq!(request.meta.gather_id, 15);
        assert_eq!(request.meta.query_ts, 12);
        assert_eq!(request.meta.local_query_id, 13);
        assert_eq!(request.meta.server_id, 21);
        assert_eq!(request.meta.mpp_version, 16);
        assert_eq!(request.meta.resource_group_name, "resource-group");
        assert_eq!(request.meta.sql_digest, vec![18]);
        assert_eq!(request.meta.plan_digest, vec![19]);
        assert_eq!(request.meta.task_id, 0);
        assert_eq!(request.meta.address, "");
        assert_eq!(request.meta.coordinator_address, "");
        assert!(!request.meta.report_execution_summary);
        assert_eq!(request.meta.connection_id, 0);
        assert_eq!(request.meta.connection_alias, "");
    }
}

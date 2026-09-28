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

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::compact_table::{
    CompactBackoff, CompactChunk, CompactError, CompactErrorKind, CompactLogEvent, CompactRequest,
    CompactResponse, CompactRpcResponse, CompactRun, CompactRuntime, CompactTableTiFlashExec,
    CompactTransportError, CompactTransportErrorKind, PartitionDefinition, PartitionInfo,
    ServerInfo, ServerType, TableInfo, TiFlashReplicaInfo,
};

type RpcResult = Result<CompactRpcResponse, CompactTransportError>;

#[derive(Default)]
struct RunState {
    requests: Vec<(String, CompactRequest)>,
    scripted: HashMap<String, VecDeque<RpcResult>>,
    cancelled: bool,
    cancel_calls: usize,
    finished: bool,
}

#[derive(Default)]
struct TestRun {
    state: Mutex<RunState>,
}

impl TestRun {
    fn script(&self, address: &str, responses: Vec<RpcResult>) {
        self.state
            .lock()
            .unwrap()
            .scripted
            .insert(address.to_owned(), responses.into());
    }

    fn requests(&self) -> Vec<(String, CompactRequest)> {
        self.state.lock().unwrap().requests.clone()
    }
}

impl CompactRun for TestRun {
    fn IsCancelled(&self) -> bool {
        self.state.lock().unwrap().cancelled
    }
    fn CancellationError(&self) -> astersql_errors::SharedError {
        astersql_errors::New("compact cancelled")
    }
    fn Cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.cancelled = true;
        state.cancel_calls += 1;
    }
    fn SendCompact(
        &self,
        address: &str,
        request: &CompactRequest,
        _timeout: Duration,
    ) -> Result<CompactRpcResponse, CompactTransportError> {
        let mut state = self.state.lock().unwrap();
        state.requests.push((address.to_owned(), request.clone()));
        state
            .scripted
            .get_mut(address)
            .unwrap_or_else(|| panic!("missing script for {address}"))
            .pop_front()
            .unwrap_or_else(|| panic!("missing response for {address}"))
    }
    fn Finish(&self) -> Result<(), astersql_errors::SharedError> {
        self.state.lock().unwrap().finished = true;
        Ok(())
    }
}

struct TestBackoff {
    calls: Arc<Mutex<Vec<String>>>,
    fail: bool,
}
impl CompactBackoff for TestBackoff {
    fn Backoff(&mut self, network_error: &str) -> Result<(), astersql_errors::SharedError> {
        self.calls.lock().unwrap().push(network_error.to_owned());
        if self.fail {
            Err(astersql_errors::New("backoff exhausted"))
        } else {
            Ok(())
        }
    }
}

struct TestRuntime {
    run: Arc<TestRun>,
    stores: Vec<ServerInfo>,
    warnings: Mutex<Vec<String>>,
    logs: Mutex<Vec<CompactLogEvent>>,
    backoffs: Arc<Mutex<Vec<String>>>,
    fail_backoff: bool,
    began: AtomicBool,
}

impl TestRuntime {
    fn new(run: Arc<TestRun>, tiflash_stores: usize) -> Arc<Self> {
        let mut stores = vec![ServerInfo {
            ServerType: ServerType::TiKV,
            Address: "tikv0".into(),
        }];
        stores.extend((0..tiflash_stores).map(|index| ServerInfo {
            ServerType: ServerType::TiFlash,
            Address: format!("tiflash{index}"),
        }));
        Arc::new(Self {
            run,
            stores,
            warnings: Mutex::new(Vec::new()),
            logs: Mutex::new(Vec::new()),
            backoffs: Arc::new(Mutex::new(Vec::new())),
            fail_backoff: false,
            began: AtomicBool::new(false),
        })
    }
}

impl CompactRuntime for TestRuntime {
    fn GetStoreServerInfo(&self) -> Result<Vec<ServerInfo>, astersql_errors::SharedError> {
        Ok(self.stores.clone())
    }
    fn AppendWarning(&self, warning: &str) {
        self.warnings.lock().unwrap().push(warning.into());
    }
    fn Log(&self, event: CompactLogEvent) {
        self.logs.lock().unwrap().push(event);
    }
    fn BeginRun(&self) -> Result<Arc<dyn CompactRun>, astersql_errors::SharedError> {
        self.began.store(true, Ordering::Release);
        Ok(self.run.clone())
    }
    fn NewBackoff(&self, _max_sleep_ms: u64) -> Box<dyn CompactBackoff> {
        Box::new(TestBackoff {
            calls: self.backoffs.clone(),
            fail: self.fail_backoff,
        })
    }
}

fn response(remaining: bool, end_key: &[u8]) -> RpcResult {
    Ok(CompactRpcResponse {
        body: Some(CompactResponse {
            HasRemaining: remaining,
            CompactedEndKey: end_key.to_vec(),
            ..Default::default()
        }),
    })
}

fn response_error(kind: CompactErrorKind, message: &str) -> RpcResult {
    Ok(CompactRpcResponse {
        body: Some(CompactResponse {
            Error: Some(CompactError {
                Kind: kind,
                Message: message.into(),
            }),
            ..Default::default()
        }),
    })
}

fn network_error(message: &str) -> RpcResult {
    Err(CompactTransportError {
        kind: CompactTransportErrorKind::Network,
        message: message.into(),
    })
}

fn executor(
    runtime: Arc<TestRuntime>,
    partition: Option<Vec<i64>>,
    requested: Vec<i64>,
) -> CompactTableTiFlashExec {
    CompactTableTiFlashExec {
        runtime,
        tableInfo: TableInfo {
            ID: 42,
            Name: "test.t".into(),
            TiFlashReplica: Some(TiFlashReplicaInfo { Count: 1 }),
            Partition: partition.map(|ids| PartitionInfo {
                Definitions: ids
                    .into_iter()
                    .map(|ID| PartitionDefinition { ID })
                    .collect(),
            }),
        },
        partitionIDs: requested,
        done: false,
    }
}

#[test]
fn compact_skips_tables_without_tiflash_and_next_runs_once() {
    let run = Arc::new(TestRun::default());
    let runtime = TestRuntime::new(run, 1);
    let mut exec = executor(runtime.clone(), None, vec![]);
    exec.tableInfo.TiFlashReplica = None;
    exec.Next(&mut CompactChunk).unwrap();
    exec.Next(&mut CompactChunk).unwrap();
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        ["compact skipped: no tiflash replica in the table"]
    );
    assert!(!runtime.began.load(Ordering::Acquire));
}

#[test]
fn compact_no_remaining_filters_non_tiflash_stores_and_finishes() {
    let run = Arc::new(TestRun::default());
    run.script("tiflash0", vec![response(false, &[0xff])]);
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime.clone(), None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    let requests = run.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "tiflash0");
    assert_eq!(
        (requests[0].1.LogicalTableId, requests[0].1.PhysicalTableId),
        (42, 42)
    );
    assert!(requests[0].1.StartKey.is_empty());
    assert!(run.state.lock().unwrap().finished);
    assert!(runtime.warnings.lock().unwrap().is_empty());
}

#[test]
fn compact_retries_network_and_continues_from_returned_page_key() {
    let run = Arc::new(TestRun::default());
    run.script(
        "tiflash0",
        vec![
            network_error("temporary network error"),
            response(true, b"next"),
            response(false, b"done"),
        ],
    );
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime.clone(), None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    let requests = run.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].1.StartKey.is_empty() && requests[1].1.StartKey.is_empty());
    assert_eq!(requests[2].1.StartKey, b"next");
    assert_eq!(
        runtime.backoffs.lock().unwrap().as_slice(),
        ["temporary network error"]
    );
}

#[test]
fn compact_busy_and_unknown_errors_match_go_warnings() {
    for (kind, detail, warning) in [
        (
            CompactErrorKind::TooManyPendingTasks,
            "busy",
            "compact on store tiflash0 failed: store is too busy",
        ),
        (
            CompactErrorKind::Unknown,
            "invalid start key",
            "compact on store tiflash0 failed: internal error (check logs for details)",
        ),
    ] {
        let run = Arc::new(TestRun::default());
        run.script("tiflash0", vec![response_error(kind, detail)]);
        let runtime = TestRuntime::new(run, 1);
        executor(runtime.clone(), None, vec![])
            .Next(&mut CompactChunk)
            .unwrap();
        assert_eq!(runtime.warnings.lock().unwrap().as_slice(), [warning]);
        assert!(runtime.logs.lock().unwrap().iter().any(|event| matches!(
            event,
            CompactLogEvent::Failure {
                physical_table_id: 42,
                ..
            }
        )));
    }
}

#[test]
fn compact_in_progress_cancels_the_whole_run() {
    let run = Arc::new(TestRun::default());
    run.script(
        "tiflash0",
        vec![response_error(
            CompactErrorKind::CompactInProgress,
            "already compacting",
        )],
    );
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime.clone(), None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    let state = run.state.lock().unwrap();
    assert!(state.cancelled && state.cancel_calls >= 1 && state.finished);
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        ["compact on store tiflash0 failed: table is compacting in progress"]
    );
}

#[test]
fn missing_physical_partition_is_skipped_and_later_partition_runs() {
    let run = Arc::new(TestRun::default());
    run.script(
        "tiflash0",
        vec![
            response_error(CompactErrorKind::PhysicalTableNotExist, "dropped"),
            response(false, &[0xcd]),
        ],
    );
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime.clone(), Some(vec![101, 102]), vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    assert_eq!(
        run.requests()
            .iter()
            .map(|(_, r)| r.PhysicalTableId)
            .collect::<Vec<_>>(),
        [101, 102]
    );
    assert!(runtime.warnings.lock().unwrap().is_empty());
    assert!(runtime.logs.lock().unwrap().iter().any(|event| matches!(
        event,
        CompactLogEvent::PhysicalTableSkipped {
            physical_table_id: 101,
            ..
        }
    )));
}

#[test]
fn specified_partitions_preserve_request_order_and_page_keys() {
    let run = Arc::new(TestRun::default());
    run.script(
        "tiflash0",
        vec![
            response(true, &[0xa0]),
            response_error(CompactErrorKind::PhysicalTableNotExist, "dropped"),
            response(false, &[0xcd]),
        ],
    );
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime, Some(vec![100, 101, 102]), vec![101, 102])
        .Next(&mut CompactChunk)
        .unwrap();
    let requests = run.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        (
            requests[0].1.PhysicalTableId,
            requests[0].1.StartKey.as_slice()
        ),
        (101, &[][..])
    );
    assert_eq!(
        (
            requests[1].1.PhysicalTableId,
            requests[1].1.StartKey.as_slice()
        ),
        (101, &[0xa0][..])
    );
    assert_eq!(
        (
            requests[2].1.PhysicalTableId,
            requests[2].1.StartKey.as_slice()
        ),
        (102, &[][..])
    );
}

#[test]
fn multiple_tiflash_stores_keep_independent_page_sequences() {
    let run = Arc::new(TestRun::default());
    run.script(
        "tiflash0",
        vec![response(true, &[0xff]), response(false, &[0xaa])],
    );
    run.script(
        "tiflash1",
        vec![response(true, &[0xc0, 0xcc]), response(false, &[0xdd])],
    );
    let runtime = TestRuntime::new(run.clone(), 2);
    executor(runtime, None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    let requests = run.requests();
    for (address, key) in [("tiflash0", vec![0xff]), ("tiflash1", vec![0xc0, 0xcc])] {
        let store = requests
            .iter()
            .filter(|(actual, _)| actual == address)
            .map(|(_, r)| r)
            .collect::<Vec<_>>();
        assert_eq!(store.len(), 2);
        assert!(store[0].StartKey.is_empty());
        assert_eq!(store[1].StartKey, key);
    }
}

#[test]
fn invalid_remaining_page_is_warned_and_stops_the_store() {
    let run = Arc::new(TestRun::default());
    run.script("tiflash0", vec![response(true, &[])]);
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime.clone(), Some(vec![101, 102]), vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    assert_eq!(run.requests().len(), 1);
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        ["compact on store tiflash0 failed: internal error (check logs for details)"]
    );
    assert!(runtime.logs.lock().unwrap().iter().any(|event| matches!(
        event,
        CompactLogEvent::InvalidPage {
            physical_table_id: 101,
            ..
        }
    )));
}

#[test]
fn non_retryable_transport_error_does_not_backoff() {
    let run = Arc::new(TestRun::default());
    run.script(
        "tiflash0",
        vec![Err(CompactTransportError {
            kind: CompactTransportErrorKind::DeadlineExceeded,
            message: "deadline exceeded".into(),
        })],
    );
    let runtime = TestRuntime::new(run, 1);
    executor(runtime.clone(), None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    assert!(runtime.backoffs.lock().unwrap().is_empty());
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        ["compact on store tiflash0 failed: deadline exceeded"]
    );
}

#[test]
fn exhausted_network_backoff_preserves_the_network_error() {
    let run = Arc::new(TestRun::default());
    run.script("tiflash0", vec![network_error("Bad network")]);
    let mut runtime = TestRuntime::new(run, 1);
    Arc::get_mut(&mut runtime).unwrap().fail_backoff = true;
    executor(runtime.clone(), None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    assert_eq!(runtime.backoffs.lock().unwrap().as_slice(), ["Bad network"]);
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        ["compact on store tiflash0 failed: Bad network"]
    );
}

#[test]
fn missing_rpc_body_is_reported_and_run_resources_are_finished() {
    let run = Arc::new(TestRun::default());
    run.script("tiflash0", vec![Ok(CompactRpcResponse { body: None })]);
    let runtime = TestRuntime::new(run.clone(), 1);
    executor(runtime.clone(), None, vec![])
        .Next(&mut CompactChunk)
        .unwrap();
    assert_eq!(
        runtime.warnings.lock().unwrap().as_slice(),
        ["compact on store tiflash0 failed: TiFlash compact response body is missing"]
    );
    assert!(run.state.lock().unwrap().finished);
}

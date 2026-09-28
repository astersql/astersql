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

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use astersql_kv as kv;
use astersql_store_copr as copr;

use crate::{InMemoryBackend, TiKVDriver};

const RPC_SERVER_BUSY: &str = "tikvclient/rpcServerBusy";

#[derive(Default)]
struct BusyMetadata {
    invalidations: AtomicUsize,
}

impl BusyMetadata {
    fn location() -> copr::KeyLocation {
        copr::KeyLocation {
            region: copr::RegionVerId::new(7, 3, 11),
            start_key: Vec::new(),
            end_key: Vec::new(),
            store: Some(copr::RegionStore {
                id: 9,
                address: "127.0.0.1:20160".to_owned(),
                labels: HashMap::new(),
            }),
            peer: Some(copr::Peer {
                id: 13,
                store_id: 9,
            }),
            ..copr::KeyLocation::default()
        }
    }
}

impl copr::RegionMetadataTransport for BusyMetadata {
    fn batch_locate_key_ranges(
        &self,
        _: &[copr::KeyRange],
        _: bool,
        _: bool,
    ) -> copr::BatchResult<Vec<copr::KeyLocation>> {
        Ok(vec![Self::location()])
    }
    fn locate_key(&self, _: &[u8]) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }
    fn locate_end_key(&self, _: &[u8]) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }
    fn locate_region_by_id(&self, _: u64) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }
    fn invalidate_region(&self, _: copr::RegionVerId) {
        self.invalidations.fetch_add(1, Ordering::AcqRel);
    }
    fn is_store_alive(&self, _: &str, _: Duration) -> bool {
        true
    }
}

struct BusyTransport {
    calls: AtomicUsize,
    busy: AtomicBool,
    closed: AtomicBool,
}

impl Default for BusyTransport {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            busy: AtomicBool::new(true),
            closed: AtomicBool::new(false),
        }
    }
}

impl copr::StandardCoprocessorTransport for BusyTransport {
    fn send_unary(
        &self,
        _: &copr::StandardCoprocessorRequest,
        _: Duration,
    ) -> copr::BatchResult<copr::CopProtocolResponse> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if self.busy.load(Ordering::Acquire) {
            // A real busy RPC has transport latency; without it the in-memory
            // transport can consume the retry budget before recovery runs.
            thread::sleep(Duration::from_millis(10));
            return Ok(copr::CopProtocolResponse {
                region_error: Some("server is busy".to_owned()),
                ..copr::CopProtocolResponse::default()
            });
        }
        Ok(copr::CopProtocolResponse {
            data: b"True".to_vec(),
            ..copr::CopProtocolResponse::default()
        })
    }
    fn send_stream(
        &self,
        _: &copr::StandardCoprocessorRequest,
        _: Duration,
    ) -> copr::BatchResult<Box<dyn copr::CoprocessorResponseStream>> {
        unreachable!("the Go parity scenario uses a unary DAG request")
    }
    fn close(&self) -> copr::BatchResult<()> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
    fn close_address(&self, _: &str) -> copr::BatchResult<()> {
        Ok(())
    }
}

struct NoopLockResolver;
impl copr::TransactionLockResolver for NoopLockResolver {
    fn resolve_locks(&self, _: &[copr::TransactionLock], _: u64) -> copr::BatchResult<()> {
        Ok(())
    }
}

fn bootstrap_query() -> kv::Request {
    kv::Request {
        Tp: kv::ReqTypeDAG,
        StartTs: 1,
        Data: b"SELECT variable_value FROM mysql.tidb WHERE variable_name=\"bootstrapped\""
            .to_vec(),
        KeyRanges: Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
            StartKey: kv::Key(Vec::new()),
            EndKey: kv::Key(Vec::new()),
        }])),
        PartitionIDAndRanges: Vec::new(),
        Concurrency: 1,
        CoprRequestRateLimit: None,
        IsolationLevel: kv::IsoLevel::SI,
        Priority: kv::PriorityHigh,
        MemTracker: None,
        KeepOrder: true,
        Desc: false,
        NotFillCache: false,
        ReplicaRead: kv::ReplicaReadType::ReplicaReadLeader,
        StoreType: kv::StoreType::TiKV,
        Cacheable: false,
        SchemaVar: 0,
        BatchCop: false,
        TaskID: 1,
        TiDBServerID: 0,
        TiKVClientReadTimeout: 1_000,
        MaxExecutionTime: 1_000,
        TxnScope: kv::GlobalTxnScope.to_owned(),
        ReadReplicaScope: String::new(),
        IsStaleness: false,
        ClosestReplicaReadAdjuster: None,
        MatchStoreLabels: Vec::new(),
        ResourceGroupTagger: None,
        Paging: kv::Paging::default(),
        RequestSource: kv::util::RequestSource::default(),
        StoreBatchSize: 0,
        ResourceGroupName: "default".to_owned(),
        LimitSize: 0,
        StoreBusyThreshold: Duration::ZERO,
        MaxKeysRead: 0,
        MaxKeysReadCounter: None,
        RunawayChecker: None,
        ResourceControlInterceptor: None,
        ConnID: 1,
        ConnAlias: "sql-fail-test".to_owned(),
    }
}

fn send_option() -> kv::ClientSendOption {
    kv::ClientSendOption {
        SessionMemTracker: None,
        EnabledRateLimitAction: false,
        EventCb: None,
        EnableCollectExecutionInfo: false,
        TiFlashReplicaRead: kv::tiflash::ReplicaRead::default(),
        AppendWarning: None,
        TryCopLiteWorker: None,
    }
}

#[test]
fn TestFailBusyServerCop() {
    let _serial = crate::test_state::global_state_guard();
    let metadata = Arc::new(BusyMetadata::default());
    let transport = Arc::new(BusyTransport::default());
    let backend = copr::NetworkBackend::from_transports(
        Arc::clone(&metadata) as Arc<dyn copr::RegionMetadataTransport>,
        Arc::clone(&transport) as Arc<dyn copr::StandardCoprocessorTransport>,
        Arc::new(NoopLockResolver) as Arc<dyn copr::TransactionLockResolver>,
    );
    let coprocessor_store = Arc::new(
        copr::Store::new(
            Arc::new(backend),
            &copr::CoprocessorCacheConfig::default(),
            false,
            false,
        )
        .unwrap(),
    );
    let mut driver = TiKVDriver::with_backend(Arc::new(InMemoryBackend::default()));
    let store = driver.Open("tikv://sql-fail-test:2379").unwrap();
    store.set_coprocessor_store_for_test(coprocessor_store);

    let busy = astersql_testkit_testfailpoint::enable(RPC_SERVER_BUSY, "return(true)");
    assert!(astersql_testkit_testfailpoint::eval_bool(RPC_SERVER_BUSY));
    let recovery_transport = Arc::clone(&transport);
    let release_busy = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        recovery_transport.busy.store(false, Ordering::Release);
        drop(busy);
    });
    let context = kv::Context::todo();
    let mut response = kv::Storage::GetClient(&store)
        .Send(
            &context,
            &bootstrap_query(),
            &() as &dyn Any,
            &send_option(),
        )
        .expect("request setup must succeed");
    let subset = response
        .Next(&context)
        .expect("busy response must be retried")
        .expect("bootstrap query must return one row");
    release_busy.join().unwrap();

    assert_eq!(subset.GetData(), b"True");
    assert!(transport.calls.load(Ordering::Acquire) > 1);
    assert!(metadata.invalidations.load(Ordering::Acquire) > 0);
    response.Close().unwrap();
    store.Close().unwrap();
    assert!(transport.closed.load(Ordering::Acquire));
}

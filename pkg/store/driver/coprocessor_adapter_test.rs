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

// 验证存储驱动把 `kv::Request` 接入标准 Coprocessor 后端时的适配行为。
//
// 单元场景使用固定 Region 元数据和可控传输，覆盖 Region 错误重试、响应迭代与关闭传播；
// 忽略的集成场景则可连接真实 PD/TiKV，验证表扫描 DAG 的端到端请求与流式响应。

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_kv as kv;
use astersql_store_copr as copr;
use protobuf::{Message, RepeatedField};

use crate::{InMemoryBackend, TiKVDriver};

struct ThresholdChecker {
    seen: Mutex<Vec<(u64, Option<kv::resourcegroup::RUDetails>)>>,
}

impl kv::resourcegroup::RunawayChecker for ThresholdChecker {
    fn BeforeCopRequest(&self, request: &mut kv::resourcegroup::CopRequest) -> Result<(), String> {
        request.priority_low = true;
        request.resource_group_name = "quarantine".into();
        request.max_execution_duration_ms = 30;
        Ok(())
    }
    fn CheckThresholds(
        &self,
        ru: Option<&kv::resourcegroup::RUDetails>,
        processed_keys: u64,
        _: Option<&str>,
    ) -> Result<(), String> {
        self.seen
            .lock()
            .unwrap()
            .push((processed_keys, ru.copied()));
        if processed_keys >= 9 || ru.is_some_and(|ru| ru.read_ru >= 4.0) {
            Err("runaway threshold exceeded".into())
        } else {
            Ok(())
        }
    }
    fn CheckAction(&self) -> kv::resourcegroup::RunawayAction {
        kv::resourcegroup::RunawayAction::CoolDown
    }
    fn ResetTotalProcessedKeys(&self) {}
}

#[derive(Debug)]
struct ProductionRUInterceptor;

impl kv::resourcegroup::CopRUInterceptor for ProductionRUInterceptor {
    fn OnRequestWait(
        &self,
        request: &kv::resourcegroup::CopRPCRequestInfo,
    ) -> Result<kv::resourcegroup::RUDetails, String> {
        assert_eq!(request.resource_group_name, "default");
        assert_eq!(request.region_id, 7);
        assert_eq!(request.store_address, "127.0.0.1:20160");
        Ok(kv::resourcegroup::RUDetails {
            read_ru: 1.0,
            write_ru: 0.0,
        })
    }

    fn OnResponseWait(
        &self,
        _request: &kv::resourcegroup::CopRPCRequestInfo,
        response: &kv::resourcegroup::CopRPCResponseInfo,
    ) -> Result<kv::resourcegroup::RUDetails, String> {
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error == "not leader")
                || response.data_bytes == b"dag-row".len()
        );
        Ok(kv::resourcegroup::RUDetails {
            read_ru: 3.0,
            write_ru: 0.0,
        })
    }
}

#[derive(Default)]
struct ProductionRUChecker {
    seen: Mutex<Vec<(f64, Option<String>)>>,
}

struct RealRUChecker {
    threshold: f64,
    seen: Mutex<Vec<f64>>,
}

impl kv::resourcegroup::RunawayChecker for RealRUChecker {
    fn BeforeCopRequest(&self, _: &mut kv::resourcegroup::CopRequest) -> Result<(), String> {
        Ok(())
    }

    fn CheckThresholds(
        &self,
        ru: Option<&kv::resourcegroup::RUDetails>,
        _: u64,
        _: Option<&str>,
    ) -> Result<(), String> {
        let total = ru.map_or(0.0, |details| details.read_ru + details.write_ru);
        self.seen.lock().unwrap().push(total);
        if total >= self.threshold {
            Err("runaway RU threshold exceeded".into())
        } else {
            Ok(())
        }
    }

    fn CheckAction(&self) -> kv::resourcegroup::RunawayAction {
        kv::resourcegroup::RunawayAction::Kill
    }

    fn ResetTotalProcessedKeys(&self) {}
}

impl kv::resourcegroup::RunawayChecker for ProductionRUChecker {
    fn BeforeCopRequest(&self, _: &mut kv::resourcegroup::CopRequest) -> Result<(), String> {
        Ok(())
    }

    fn CheckThresholds(
        &self,
        ru: Option<&kv::resourcegroup::RUDetails>,
        _: u64,
        error: Option<&str>,
    ) -> Result<(), String> {
        let read_ru = ru.map_or(0.0, |details| details.read_ru);
        self.seen
            .lock()
            .unwrap()
            .push((read_ru, error.map(str::to_owned)));
        if read_ru >= 8.0 {
            Err("runaway threshold exceeded".into())
        } else {
            Ok(())
        }
    }

    fn CheckAction(&self) -> kv::resourcegroup::RunawayAction {
        kv::resourcegroup::RunawayAction::Kill
    }

    fn ResetTotalProcessedKeys(&self) {}
}

#[test]
fn kv_request_runaway_checker_reaches_cop_wire_and_response_thresholds() {
    let checker = Arc::new(ThresholdChecker {
        seen: Mutex::new(Vec::new()),
    });
    let mut request = dag_request();
    request.RunawayChecker = Some(checker.clone());
    let mapped = crate::kv_adapter::cop_request(&request).unwrap();
    let cop_checker = mapped.runaway_checker.unwrap();
    assert_eq!(cop_checker.check_action(), copr::RunawayAction::CoolDown);
    let mut wire = copr::CopWireRequest {
        priority: copr::Priority::Normal,
        resource_group_name: "original".into(),
        ..Default::default()
    };
    cop_checker.before_cop_request(&mut wire).unwrap();
    assert_eq!(wire.priority, copr::Priority::Low);
    assert_eq!(wire.resource_group_name, "quarantine");
    assert_eq!(wire.max_execution_duration_ms, 30);
    cop_checker.check_thresholds(None, 8, None).unwrap();
    assert!(matches!(
        cop_checker.check_thresholds(None, 9, None),
        Err(copr::BatchError::QueryInterrupted)
    ));
    let ru = copr::CopRUDetails {
        read_ru: 4.0,
        write_ru: 0.0,
    };
    assert!(matches!(
        cop_checker.check_thresholds(Some(&ru), 0, None),
        Err(copr::BatchError::QueryInterrupted)
    ));
    assert_eq!(
        checker.seen.lock().unwrap().last(),
        Some(&(
            0,
            Some(kv::resourcegroup::RUDetails {
                read_ru: 4.0,
                write_ru: 0.0
            })
        ))
    );
}

#[derive(Default)]
/// 提供固定路由，并记录因 Region 错误而失效的 Region。
struct DagMetadata {
    invalidated: Mutex<Vec<copr::RegionVerId>>,
}

impl DagMetadata {
    fn location() -> copr::KeyLocation {
        copr::KeyLocation {
            region: copr::RegionVerId::new(7, 3, 11),
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
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

impl copr::RegionMetadataTransport for DagMetadata {
    fn batch_locate_key_ranges(
        &self,
        _ranges: &[copr::KeyRange],
        _need_leader: bool,
        _need_buckets: bool,
    ) -> copr::BatchResult<Vec<copr::KeyLocation>> {
        Ok(vec![Self::location()])
    }

    fn locate_key(&self, _key: &[u8]) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }

    fn locate_end_key(&self, _key: &[u8]) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }

    fn locate_region_by_id(&self, _region_id: u64) -> copr::BatchResult<copr::KeyLocation> {
        Ok(Self::location())
    }

    fn invalidate_region(&self, region: copr::RegionVerId) {
        self.invalidated.lock().unwrap().push(region);
    }

    fn is_store_alive(&self, _address: &str, _ttl: Duration) -> bool {
        true
    }
}

#[derive(Default)]
/// 可控的标准 Coprocessor 传输：首次返回 Region 错误，重试后返回数据。
struct DagTransport {
    unary_calls: AtomicUsize,
    closed: AtomicBool,
}

struct EmptyRpcStream;

/// 本组测试不涉及事务锁，解析器只需保持成功语义。
struct NoopLockResolver;

impl copr::TransactionLockResolver for NoopLockResolver {
    fn resolve_locks(
        &self,
        _locks: &[copr::TransactionLock],
        _caller_start_ts: u64,
    ) -> copr::BatchResult<()> {
        Ok(())
    }
}

impl copr::CoprocessorResponseStream for EmptyRpcStream {
    fn next(&mut self) -> copr::BatchResult<Option<copr::CopProtocolResponse>> {
        Ok(None)
    }

    fn close(&mut self) -> copr::BatchResult<()> {
        Ok(())
    }
}

impl copr::StandardCoprocessorTransport for DagTransport {
    fn send_unary(
        &self,
        request: &copr::StandardCoprocessorRequest,
        _timeout: Duration,
    ) -> copr::BatchResult<copr::CopProtocolResponse> {
        assert_eq!(request.region, copr::RegionVerId::new(7, 3, 11));
        assert_eq!(request.peer.as_ref().map(|peer| peer.id), Some(13));
        assert_eq!(request.address, "127.0.0.1:20160");
        let call = self.unary_calls.fetch_add(1, Ordering::AcqRel);
        // 首次请求模拟 leader 已变化，驱动应使旧 Region 失效并按新定位重试。
        Ok(if call == 0 {
            copr::CopProtocolResponse {
                region_error: Some("not leader".to_owned()),
                ..copr::CopProtocolResponse::default()
            }
        } else {
            copr::CopProtocolResponse {
                data: b"dag-row".to_vec(),
                ..copr::CopProtocolResponse::default()
            }
        })
    }

    fn send_stream(
        &self,
        _request: &copr::StandardCoprocessorRequest,
        _timeout: Duration,
    ) -> copr::BatchResult<Box<dyn copr::CoprocessorResponseStream>> {
        Ok(Box::new(EmptyRpcStream))
    }

    fn close(&self) -> copr::BatchResult<()> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }

    fn close_address(&self, _address: &str) -> copr::BatchResult<()> {
        Ok(())
    }
}

/// 构造字段完整且取值稳定的标准 DAG 请求，便于观察适配过程是否保留请求语义。
fn dag_request() -> kv::Request {
    kv::Request {
        Tp: kv::ReqTypeDAG,
        StartTs: 42,
        Data: b"standard-dag".to_vec(),
        KeyRanges: Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
            StartKey: kv::Key(b"k1".to_vec()),
            EndKey: kv::Key(b"k9".to_vec()),
        }])),
        PartitionIDAndRanges: Vec::new(),
        Concurrency: 1,
        CoprRequestLimiter: None,
        QueryCopStoreLimiter: None,
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
        TaskID: 99,
        TiDBServerID: 0,
        TxnScope: kv::GlobalTxnScope.to_owned(),
        ReadReplicaScope: String::new(),
        IsStaleness: false,
        ClosestReplicaReadAdjuster: None,
        MatchStoreLabels: Vec::new(),
        ResourceGroupTagger: None,
        Paging: kv::Paging::default(),
        RequestSource: kv::util::RequestSource::default(),
        StoreBatchSize: 0,
        AllowBatchTaskDataMerge: false,
        ExecuteBatchTasksSerially: false,
        ResourceGroupName: "default".to_owned(),
        LimitSize: 0,
        StoreBusyThreshold: Duration::ZERO,
        TiKVClientReadTimeout: 1_000,
        MaxExecutionTime: 1_000,
        MaxKeysRead: 0,
        MaxKeysReadCounter: None,
        RunawayChecker: None,
        ResourceControlInterceptor: None,
        ConnID: 123,
        ConnAlias: "unit-test".to_owned(),
    }
}

#[test]
fn tiflash_batch_dag_request_preserves_store_type() {
    let mut request = dag_request();
    request.StoreType = kv::StoreType::TiFlash;
    request.BatchCop = true;
    let adapted = super::kv_adapter::cop_request(&request).expect("TiFlash batch DAG request");
    assert_eq!(adapted.store_type, copr::StoreType::TiFlash);
    assert!(adapted.batch_cop);
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

/// 生成与 TiDB 表记录键编码一致的有符号整数片段。
fn encode_int(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

/// 构造表记录键 `t{table_id}_r{handle}`，供真实 TiKV 场景限定扫描范围。
fn table_record_key(table_id: i64, handle: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(19);
    key.push(b't');
    key.extend_from_slice(&encode_int(table_id));
    key.extend_from_slice(b"_r");
    key.extend_from_slice(&encode_int(handle));
    key
}

/// 序列化只读取主键句柄列的最小 TableScan DAG。
fn table_scan_dag(table_id: i64) -> Vec<u8> {
    let mut handle_column = tipb::ColumnInfo::new();
    handle_column.set_column_id(1);
    handle_column.set_tp(8); // MySQL TypeLonglong 类型编号。
    handle_column.set_pk_handle(true);

    let mut scan = tipb::TableScan::new();
    scan.set_table_id(table_id);
    scan.set_columns(RepeatedField::from_vec(vec![handle_column]));

    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeTableScan);
    executor.set_tbl_scan(scan);

    let mut dag = tipb::DagRequest::new();
    dag.set_executors(RepeatedField::from_vec(vec![executor]));
    dag.set_output_offsets(vec![0]);
    dag.write_to_bytes().expect("DAG protobuf must serialize")
}

#[test]
/// 使用可控后端验证请求路由、Region 重试、响应结束语义和关闭传播。
fn canonical_dag_routes_retries_streams_and_closes() {
    let metadata = Arc::new(DagMetadata::default());
    let transport = Arc::new(DagTransport::default());
    let backend = copr::NetworkBackend::from_transports(
        Arc::clone(&metadata) as Arc<dyn copr::RegionMetadataTransport>,
        Arc::clone(&transport) as Arc<dyn copr::StandardCoprocessorTransport>,
        Arc::new(NoopLockResolver) as Arc<dyn copr::TransactionLockResolver>,
    );
    let backend: Arc<dyn copr::StoreBackend> = Arc::new(backend);
    let coprocessor_store = Arc::new(
        copr::Store::new(
            backend,
            &copr::CoprocessorCacheConfig::default(),
            false,
            false,
        )
        .unwrap(),
    );

    let mut driver = TiKVDriver::with_backend(Arc::new(InMemoryBackend::default()));
    let store = driver.Open("tikv://dag-adapter-test:2379").unwrap();
    store.set_coprocessor_store_for_test(Arc::clone(&coprocessor_store));
    let client = kv::Storage::GetClient(&store);
    assert!(client.IsRequestTypeSupported(kv::ReqTypeAnalyze, kv::ReqSubTypeBasic));

    let ctx = kv::Context::todo();
    let request = dag_request();
    let mut response = client
        .Send(&ctx, &request, &() as &dyn Any, &send_option())
        .expect("configured client response");
    let subset = response
        .Next(&ctx)
        .expect("region retry must succeed")
        .expect("DAG must return one row packet");
    assert_eq!(subset.GetData(), b"dag-row");
    response
        .Close()
        .expect("explicit stream close must succeed");
    assert!(response.Next(&ctx).unwrap().is_none());
    assert_eq!(transport.unary_calls.load(Ordering::Acquire), 2);
    assert!(!metadata.invalidated.lock().unwrap().is_empty());

    store.Close().unwrap();
    assert!(transport.closed.load(Ordering::Acquire));
}

#[test]
fn production_cop_resource_control_ru_reaches_runaway_across_retry() {
    let metadata = Arc::new(DagMetadata::default());
    let transport = Arc::new(DagTransport::default());
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
    let store = driver.Open("tikv://dag-ru-adapter-test:2379").unwrap();
    store.set_coprocessor_store_for_test(coprocessor_store);

    let checker = Arc::new(ProductionRUChecker::default());
    let mut request = dag_request();
    request.RunawayChecker = Some(checker.clone());
    request.ResourceControlInterceptor = Some(Arc::new(ProductionRUInterceptor));
    let mut response = kv::Storage::GetClient(&store)
        .Send(
            &kv::Context::todo(),
            &request,
            &() as &dyn Any,
            &send_option(),
        )
        .unwrap();
    assert!(response.Next(&kv::Context::todo()).is_err());
    assert_eq!(
        *checker.seen.lock().unwrap(),
        vec![
            (4.0, Some("transport error: not leader".to_owned())),
            (8.0, None),
        ],
    );
    assert_eq!(transport.unary_calls.load(Ordering::Acquire), 2);
}

#[test]
#[ignore = "requires REAL_TIKV_PD and a running PD/TiKV cluster"]
/// 在显式提供真实集群时，验证写入的表记录可由标准 DAG 流式读取。
fn real_standard_dag_coprocessor_routes_and_streams() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD must name the real PD endpoint");
    let mut driver = TiKVDriver::default();
    let store = driver
        .Open(&format!("tikv://{pd}?disableGC=true"))
        .expect("real TiKV store and standard DAG transport must open");
    let ctx = kv::Context::todo();
    let suffix = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .expect("real PD must allocate a timestamp")
        .Ver;
    let table_id = 8_000_000_i64 + i64::try_from(suffix % 1_000_000).unwrap();
    let handle = 42_i64;
    let record_key = table_record_key(table_id, handle);

    let mut transaction = kv::Storage::Begin(&store, &[]).expect("transaction must begin");
    kv::Mutator::Set(
        transaction.as_mut(),
        kv::Key(record_key.clone()),
        vec![0x80],
    )
    .expect("table key must be written");
    transaction.Commit(&ctx).expect("table key must commit");

    let build_request = |checker: Arc<RealRUChecker>| {
        let mut request = dag_request();
        request.StartTs = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
            .expect("DAG start_ts must be allocated")
            .Ver;
        request.Data = table_scan_dag(table_id);
        request.KeyRanges = Some(kv::NewNonPartitionedKeyRanges(vec![kv::KeyRange {
            StartKey: kv::Key(record_key.clone()),
            EndKey: kv::Key(table_record_key(table_id, handle + 1)),
        }]));
        request.RunawayChecker = Some(checker);
        request
    };
    let allowed_checker = Arc::new(RealRUChecker {
        threshold: f64::MAX,
        seen: Mutex::new(Vec::new()),
    });
    let request = build_request(allowed_checker.clone());
    let mut response = kv::Storage::GetClient(&store)
        .Send(&ctx, &request, &(), &send_option())
        .expect("configured client response");
    let mut packets = 0_usize;
    let mut bytes = 0_usize;
    while let Some(subset) = response.Next(&ctx).expect("real standard DAG must stream") {
        packets += 1;
        bytes += subset.GetData().len();
    }
    response.Close().expect("real DAG stream must close");
    assert!(packets > 0, "standard DAG must return at least one packet");
    assert!(
        bytes > 0,
        "standard DAG packet must contain a SelectResponse"
    );
    let read_ru = allowed_checker
        .seen
        .lock()
        .unwrap()
        .iter()
        .copied()
        .fold(0.0, f64::max);
    assert!(read_ru > 0.0, "production client must report non-zero RU");

    let killed_checker = Arc::new(RealRUChecker {
        threshold: read_ru / 2.0,
        seen: Mutex::new(Vec::new()),
    });
    let request = build_request(killed_checker.clone());
    let mut killed_response = kv::Storage::GetClient(&store)
        .Send(&ctx, &request, &(), &send_option())
        .expect("kill-threshold response must be created");
    let kill_error = match killed_response.Next(&ctx) {
        Err(error) => error,
        Ok(_) => panic!("RU below the measured consumption must kill the request"),
    };
    killed_response
        .Close()
        .expect("killed real DAG stream must close");
    assert!(
        kill_error.to_string().contains("query interrupted"),
        "unexpected kill error: {kill_error}"
    );

    let mut cleanup = kv::Storage::Begin(&store, &[]).expect("cleanup transaction must begin");
    kv::Mutator::Delete(cleanup.as_mut(), kv::Key(record_key))
        .expect("test table key must be deleted");
    cleanup
        .Commit(&ctx)
        .expect("test table key deletion must commit");
    println!(
        "standard_dag packets={packets} bytes={bytes} read_ru={read_ru:.6} \
         allow_action=completed kill_action={kill_error} streams_closed=true table_id={table_id}"
    );
    store.Close().expect("real TiKV store must close");
}

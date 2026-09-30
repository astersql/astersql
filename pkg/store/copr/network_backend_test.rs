// Copyright 2026 AsterSQL.

// 网络协处理器后端的端到端单元测试。
//
// 通过可记录调用的传输层替身，验证 Region 元数据路由、错误失效、流式响应关闭、
// 事务锁解析，以及不同 KeyCodec 下的物理键边界与逻辑键还原。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kvproto::kvrpcpb;
use protobuf::Message;

use crate::batch_request_sender::{BatchResult, KeyRange, Peer, RegionVerId, Store};
use crate::coprocessor::{CopProtocolResponse, CopRequestAttemptLimiter, CopTask, CopWireRequest};
use crate::network_backend::{
    CoprocessorResponseStream, KeyCodec, NetworkBackend, RegionMetadataTransport,
    StandardCoprocessorRequest, StandardCoprocessorTransport, TransactionLock,
    TransactionLockResolver,
};
use crate::region_cache::{KeyLocation, RegionCacheBackend};
use crate::store::StoreBackend;

#[derive(Default)]
/// 固定返回新 Region 路由，并记录缓存失效与按 ID 定位次数的元数据替身。
struct RecordingMetadata {
    invalidated: Mutex<Vec<RegionVerId>>,
    locate_by_id: AtomicUsize,
}

impl RecordingMetadata {
    fn location() -> KeyLocation {
        KeyLocation {
            region: RegionVerId::new(7, 3, 11),
            start_key: b"a".to_vec(),
            end_key: b"z".to_vec(),
            store: Some(Store {
                id: 9,
                address: "127.0.0.1:20160".to_owned(),
                labels: HashMap::new(),
            }),
            peer: Some(Peer {
                id: 13,
                store_id: 9,
            }),
            ..KeyLocation::default()
        }
    }
}

impl RegionMetadataTransport for RecordingMetadata {
    fn batch_locate_key_ranges(
        &self,
        _ranges: &[KeyRange],
        _need_leader: bool,
        _need_buckets: bool,
    ) -> BatchResult<Vec<KeyLocation>> {
        Ok(vec![Self::location()])
    }

    fn locate_key(&self, _key: &[u8]) -> BatchResult<KeyLocation> {
        Ok(Self::location())
    }

    fn locate_end_key(&self, _key: &[u8]) -> BatchResult<KeyLocation> {
        Ok(Self::location())
    }

    fn locate_region_by_id(&self, _region_id: u64) -> BatchResult<KeyLocation> {
        self.locate_by_id.fetch_add(1, Ordering::AcqRel);
        Ok(Self::location())
    }

    fn read_replicas(&self, _region_id: u64) -> BatchResult<Vec<KeyLocation>> {
        Ok((0..3)
            .map(|offset| {
                let mut location = Self::location();
                let store = location.store.as_mut().unwrap();
                store.id += offset;
                store.address = format!("127.0.0.1:{}", 20160 + offset);
                let peer = location.peer.as_mut().unwrap();
                peer.id += offset;
                peer.store_id += offset;
                location
            })
            .collect())
    }

    fn invalidate_region(&self, region: RegionVerId) {
        self.invalidated.lock().unwrap().push(region);
    }

    fn is_store_alive(&self, address: &str, _ttl: Duration) -> bool {
        address == "127.0.0.1:20160"
    }
}

struct RecordingStream {
    responses: Vec<CopProtocolResponse>,
    closed: Arc<AtomicBool>,
}

impl CoprocessorResponseStream for RecordingStream {
    fn next(&mut self) -> BatchResult<Option<CopProtocolResponse>> {
        Ok(if self.responses.is_empty() {
            None
        } else {
            Some(self.responses.remove(0))
        })
    }

    fn close(&mut self) -> BatchResult<()> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

#[derive(Default)]
/// 记录发出的请求，并用首次 Region 错误、后续成功响应模拟重试链路。
struct RecordingCoprocessor {
    requests: Mutex<Vec<StandardCoprocessorRequest>>,
    unary_calls: AtomicUsize,
    stream_closed: Arc<AtomicBool>,
    admission: Option<Arc<astersql_kv::CoprRequestLimiter>>,
}

#[derive(Default)]
/// 保存每次锁解析的锁集合和调用方时间戳，供测试核对协议字段转换。
struct RecordingLockResolver {
    calls: Mutex<Vec<(Vec<TransactionLock>, u64)>>,
}

impl TransactionLockResolver for RecordingLockResolver {
    fn resolve_locks(&self, locks: &[TransactionLock], caller_start_ts: u64) -> BatchResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push((locks.to_vec(), caller_start_ts));
        Ok(())
    }
}

impl StandardCoprocessorTransport for RecordingCoprocessor {
    fn send_unary(
        &self,
        request: &StandardCoprocessorRequest,
        _timeout: Duration,
    ) -> BatchResult<CopProtocolResponse> {
        if let Some(limiter) = &self.admission {
            assert!(!limiter.TryAcquire(), "RPC attempt must hold the permit");
        }
        self.requests.lock().unwrap().push(request.clone());
        let call = self.unary_calls.fetch_add(1, Ordering::AcqRel);
        Ok(if call == 0 {
            CopProtocolResponse {
                region_error: Some("not leader".to_owned()),
                ..CopProtocolResponse::default()
            }
        } else {
            CopProtocolResponse {
                data: b"dag-row".to_vec(),
                ..CopProtocolResponse::default()
            }
        })
    }

    fn send_stream(
        &self,
        request: &StandardCoprocessorRequest,
        _timeout: Duration,
    ) -> BatchResult<Box<dyn CoprocessorResponseStream>> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(Box::new(RecordingStream {
            responses: vec![
                CopProtocolResponse {
                    data: b"stream-row-1".to_vec(),
                    ..CopProtocolResponse::default()
                },
                CopProtocolResponse {
                    data: b"stream-row-2".to_vec(),
                    ..CopProtocolResponse::default()
                },
            ],
            closed: Arc::clone(&self.stream_closed),
        }))
    }

    fn close(&self) -> BatchResult<()> {
        Ok(())
    }

    fn close_address(&self, _address: &str) -> BatchResult<()> {
        Ok(())
    }
}

#[test]
fn go_merge_48_network_send_holds_and_releases_attempt_permit() {
    let metadata = Arc::new(RecordingMetadata::default());
    let limiter = astersql_kv::NewCoprRequestLimiter(1).unwrap();
    let coprocessor = Arc::new(RecordingCoprocessor {
        admission: Some(Arc::clone(&limiter)),
        ..RecordingCoprocessor::default()
    });
    let backend = NetworkBackend::from_transports(
        metadata as Arc<dyn RegionMetadataTransport>,
        coprocessor as Arc<dyn StandardCoprocessorTransport>,
        Arc::new(RecordingLockResolver::default()) as Arc<dyn TransactionLockResolver>,
    );
    let task = CopTask {
        region: RegionVerId::new(7, 2, 10),
        ..CopTask::default()
    };
    let wire = CopWireRequest {
        attempt_limiter: Some(Arc::new(CopRequestAttemptLimiter::new(
            Some(Arc::clone(&limiter)),
            None,
            None,
        ))),
        ..CopWireRequest::default()
    };
    StoreBackend::send_coprocessor(&backend, &task, &wire).unwrap();
    assert!(limiter.TryAcquire(), "permit must be released after send");
    limiter.Release();
}

#[test]
fn network_backend_routes_context_invalidates_region_and_closes_stream() {
    let metadata = Arc::new(RecordingMetadata::default());
    let coprocessor = Arc::new(RecordingCoprocessor::default());
    let lock_resolver = Arc::new(RecordingLockResolver::default());
    let backend = NetworkBackend::from_transports(
        Arc::clone(&metadata) as Arc<dyn RegionMetadataTransport>,
        Arc::clone(&coprocessor) as Arc<dyn StandardCoprocessorTransport>,
        lock_resolver as Arc<dyn TransactionLockResolver>,
    );
    let task = CopTask {
        region: RegionVerId::new(7, 2, 10),
        store_address: "stale:20160".to_owned(),
        client_read_timeout: Duration::from_secs(2),
        ..CopTask::default()
    };
    let wire = CopWireRequest {
        start_ts: 42,
        data: b"standard-dag".to_vec(),
        ranges: vec![KeyRange {
            start: b"k1".to_vec(),
            end: b"k9".to_vec(),
        }],
        ..CopWireRequest::default()
    };

    // 即使收到 Region 错误也应保留协议响应，同时使任务携带的旧 Region 缓存失效。
    let first = StoreBackend::send_coprocessor(&backend, &task, &wire)
        .expect("region error must remain a protocol response");
    assert_eq!(first.region_error.as_deref(), Some("not leader"));
    let invalidated = metadata.invalidated.lock().unwrap();
    assert_eq!(invalidated.len(), 2);
    assert!(
        invalidated
            .iter()
            .all(|region| *region == RegionVerId::new(7, 2, 10))
    );
    drop(invalidated);

    // 再次发送时必须重新定位 Region，并把新 leader、peer 与原请求上下文写入传输请求。
    let second = StoreBackend::send_coprocessor(&backend, &task, &wire)
        .expect("retry must use the freshly located leader");
    assert_eq!(second.data, b"dag-row");
    let requests = coprocessor.requests.lock().unwrap();
    assert_eq!(requests[0].address, "127.0.0.1:20160");
    assert_eq!(requests[0].region, RegionVerId::new(7, 3, 11));
    assert_eq!(requests[0].peer.as_ref().map(|peer| peer.id), Some(13));
    assert_eq!(requests[0].wire.start_ts, 42);
    drop(requests);

    // 流式路径同样重新定位 Region，并将显式关闭传递到底层响应流。
    let mut stream = StoreBackend::send_coprocessor_stream(&backend, &task, &wire)
        .expect("standard coprocessor stream must open");
    assert_eq!(
        stream.next().unwrap().unwrap().data,
        b"stream-row-1".to_vec()
    );
    assert_eq!(
        stream.next().unwrap().unwrap().data,
        b"stream-row-2".to_vec()
    );
    assert!(stream.next().unwrap().is_none());
    stream.close().expect("stream close must reach transport");
    assert!(coprocessor.stream_closed.load(Ordering::Acquire));
    assert_eq!(metadata.locate_by_id.load(Ordering::Acquire), 3);
}

#[test]
fn network_backend_resolves_regular_and_shared_locks_like_go() {
    let metadata = Arc::new(RecordingMetadata::default());
    let coprocessor = Arc::new(RecordingCoprocessor::default());
    let lock_resolver = Arc::new(RecordingLockResolver::default());
    let backend = NetworkBackend::from_transports(
        metadata as Arc<dyn RegionMetadataTransport>,
        coprocessor as Arc<dyn StandardCoprocessorTransport>,
        Arc::clone(&lock_resolver) as Arc<dyn TransactionLockResolver>,
    );

    // 普通 LockInfo 应完整映射为一个 TransactionLock，不能丢失异步提交等扩展字段。
    let mut regular = kvrpcpb::LockInfo::new();
    regular.set_primary_lock(b"regular-primary".to_vec());
    regular.set_lock_version(101);
    regular.set_key(b"regular-key".to_vec());
    regular.set_lock_ttl(3_000);
    regular.set_txn_size(2);
    regular.set_lock_type(kvrpcpb::Op::Put);
    regular.set_lock_for_update_ts(99);
    regular.set_use_async_commit(true);
    regular.set_min_commit_ts(111);
    regular.set_secondaries(vec![b"secondary".to_vec()].into());
    regular.set_duration_to_last_update_ms(7);
    regular.set_is_txn_file(true);
    StoreBackend::resolve_lock(&backend, &regular.write_to_bytes().unwrap(), 500).unwrap();

    // 共享锁包装会被展开为同一次解析调用中的多个独立锁。
    let mut shared_one = kvrpcpb::LockInfo::new();
    shared_one.set_primary_lock(b"shared-primary-1".to_vec());
    shared_one.set_lock_version(201);
    shared_one.set_key(b"shared-key-1".to_vec());
    shared_one.set_lock_ttl(4_000);
    let mut shared_two = kvrpcpb::LockInfo::new();
    shared_two.set_primary_lock(b"shared-primary-2".to_vec());
    shared_two.set_lock_version(202);
    shared_two.set_key(b"shared-key-2".to_vec());
    shared_two.set_lock_ttl(5_000);
    let mut shared_wrapper = kvrpcpb::LockInfo::new();
    shared_wrapper.set_shared_lock_infos(vec![shared_one, shared_two].into());
    StoreBackend::resolve_lock(&backend, &shared_wrapper.write_to_bytes().unwrap(), 600).unwrap();

    let calls = lock_resolver.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, 500);
    assert_eq!(calls[0].0.len(), 1);
    assert_eq!(
        calls[0].0[0],
        TransactionLock {
            primary_lock: b"regular-primary".to_vec(),
            lock_version: 101,
            key: b"regular-key".to_vec(),
            lock_ttl: 3_000,
            txn_size: 2,
            lock_type: kvrpcpb::Op::Put as i32,
            lock_for_update_ts: 99,
            use_async_commit: true,
            min_commit_ts: 111,
            secondaries: vec![b"secondary".to_vec()],
            duration_to_last_update_ms: 7,
            is_txn_file: true,
        }
    );
    assert_eq!(calls[1].1, 600);
    assert_eq!(
        calls[1]
            .0
            .iter()
            .map(|lock| (lock.lock_version, lock.key.clone()))
            .collect::<Vec<_>>(),
        vec![
            (201, b"shared-key-1".to_vec()),
            (202, b"shared-key-2".to_vec())
        ]
    );

    // V2 keyspace 前缀只属于物理存储键，交给锁解析器前必须递归剥离。
    let v2 = KeyCodec::v2("tenant".to_owned(), 0x010203).unwrap();
    let encode = |key: &[u8]| v2.encode_range(key, b"").0;
    let mut physical = kvrpcpb::LockInfo::new();
    physical.set_key(encode(b"row"));
    physical.set_primary_lock(encode(b"primary"));
    physical.set_secondaries(vec![encode(b"secondary-1"), encode(b"secondary-2")].into());
    let mut physical_shared = kvrpcpb::LockInfo::new();
    physical_shared.set_shared_lock_infos(vec![physical].into());
    v2.decode_lock_info(&mut physical_shared).unwrap();
    let logical = &physical_shared.get_shared_lock_infos()[0];
    assert_eq!(logical.get_key(), b"row");
    assert_eq!(logical.get_primary_lock(), b"primary");
    assert_eq!(
        logical.get_secondaries(),
        [b"secondary-1".to_vec(), b"secondary-2".to_vec()]
    );
}

#[test]
fn transaction_region_codec_matches_memcomparable_and_keyspace_boundaries() {
    // V1 Region 键使用 memcomparable 编码；空结束键仍表示无上界。
    let v1 = KeyCodec::v1();
    let (encoded_start, encoded_end) = v1.encode_region_range(b"123", b"12345678");
    assert_eq!(encoded_start, vec![b'1', b'2', b'3', 0, 0, 0, 0, 0, 0xfa]);
    assert_eq!(
        encoded_end,
        vec![
            b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0xf7
        ]
    );
    assert_eq!(
        v1.decode_region_range(&encoded_start, &encoded_end)
            .unwrap(),
        (b"123".to_vec(), b"12345678".to_vec())
    );
    assert!(
        v1.encode_end_region_key(b"").is_empty(),
        "unbounded reverse scan must ask PD for the final V1 Region"
    );

    // V2 无界结束键提升到下一 keyspace 前缀，避免越过租户边界继续扫描。
    let v2 = KeyCodec::v2("tenant".to_owned(), 0x010203).unwrap();
    let (cop_start, cop_end) = v2.encode_range(b"row", b"");
    assert_eq!(cop_start, [b"x\x01\x02\x03".as_slice(), b"row"].concat());
    assert_eq!(cop_end, b"x\x01\x02\x04");
    let (region_start, region_end) = v2.encode_region_range(b"row", b"");
    assert_eq!(
        v2.decode_region_range(&region_start, &region_end).unwrap(),
        (b"row".to_vec(), Vec::new())
    );
    assert_eq!(v2.encode_end_region_key(b""), region_end);
}

#[derive(Default)]
struct DeadlineCoprocessor {
    attempts: Mutex<Vec<(u64, Duration)>>,
}

impl StandardCoprocessorTransport for DeadlineCoprocessor {
    fn send_unary(
        &self,
        request: &StandardCoprocessorRequest,
        timeout: Duration,
    ) -> BatchResult<CopProtocolResponse> {
        self.attempts
            .lock()
            .unwrap()
            .push((request.peer.as_ref().unwrap().store_id, timeout));
        if timeout == Duration::from_millis(1) {
            return Err(crate::batch_request_sender::BatchError::Transport(
                "DEADLINE_EXCEEDED".into(),
            ));
        }
        Ok(CopProtocolResponse {
            data: b"actual response".to_vec(),
            ..Default::default()
        })
    }
    fn send_stream(
        &self,
        _: &StandardCoprocessorRequest,
        _: Duration,
    ) -> BatchResult<Box<dyn CoprocessorResponseStream>> {
        unreachable!("unary read")
    }
    fn close(&self) -> BatchResult<()> {
        Ok(())
    }
    fn close_address(&self, _: &str) -> BatchResult<()> {
        Ok(())
    }
}

#[test]
fn read_timeout_tries_three_replicas_before_default_timeout() {
    let transport = Arc::new(DeadlineCoprocessor::default());
    let backend = NetworkBackend::from_transports(
        Arc::new(RecordingMetadata::default()),
        transport.clone(),
        Arc::new(RecordingLockResolver::default()),
    );
    let stats = Arc::new(tikv_client::ReadStats::default());
    let task = CopTask {
        region: RecordingMetadata::location().region,
        client_read_timeout: Duration::from_millis(1),
        read_stats: Some(stats.clone()),
        ..Default::default()
    };
    let response =
        StoreBackend::send_coprocessor(&backend, &task, &CopWireRequest::default()).unwrap();
    assert_eq!(response.data, b"actual response");
    let short = Duration::from_millis(1);
    assert_eq!(
        *transport.attempts.lock().unwrap(),
        vec![
            (9, short),
            (10, short),
            (11, short),
            (9, Duration::from_secs(60))
        ]
    );
    let attempts = stats.snapshot();
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt.store_id)
            .collect::<Vec<_>>(),
        vec![9, 10, 11, 9]
    );
    assert!(attempts[..3].iter().all(|attempt| attempt.timed_out));
    assert!(!attempts[3].timed_out);
}

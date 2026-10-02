// Copyright 2026 AsterSQL.

use super::*;
use astersql_kv as kv;

#[test]
fn invalid_input_is_rejected_before_connecting() {
    assert!(write_and_ingest(&[], None, 0, vec![]).is_err());
    assert!(
        write_and_ingest(
            &[],
            None,
            1,
            vec![(b"b".to_vec(), vec![]), (b"a".to_vec(), vec![])]
        )
        .is_err()
    );
    assert!(
        write_and_ingest(
            &[],
            None,
            1,
            vec![(b"a".to_vec(), vec![]), (b"a".to_vec(), vec![])]
        )
        .is_err()
    );
    assert_eq!(
        write_and_ingest(&[], None, 1, vec![]).unwrap().write_rpcs,
        0
    );
}

#[test]
fn pd_keys_match_memcomparable_boundary_encoding() {
    assert_eq!(region_key(b""), vec![0, 0, 0, 0, 0, 0, 0, 0, 247]);
    assert_eq!(
        region_key(b"12345678"),
        [b"12345678".as_slice(), &[255, 0, 0, 0, 0, 0, 0, 0, 0, 247]].concat()
    );
    assert!(region_key(b"12345678") < region_key(b"12345678\0"));
}

#[test]
#[ignore = "requires REAL_TIKV_PD three-node cluster"]
fn real_sst_write_and_multi_ingest_preserve_mvcc_visibility() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD required");
    let mut driver = crate::TiKVDriver::default();
    let store = driver.Open(&format!("tikv://{pd}?disableGC=true")).unwrap();
    let old_version = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope).unwrap();
    let prefix = format!("astersql/sst-import/{}/", old_version.Ver);
    let keys = [
        format!("{prefix}a").into_bytes(),
        format!("{prefix}b").into_bytes(),
    ];
    let old = kv::Storage::GetSnapshot(&store, old_version);
    let commit_ts = kv::Storage::CurrentVersion(&store, kv::GlobalTxnScope)
        .unwrap()
        .Ver;
    let stats = write_and_ingest(
        &store.GetPDAddrs().unwrap(),
        store.TLSConfig(),
        commit_ts,
        vec![
            (keys[0].clone(), b"first".to_vec()),
            (keys[1].clone(), b"second".to_vec()),
        ],
    )
    .unwrap();
    assert_eq!(stats.keys, 2);
    assert!(
        stats.write_rpcs >= 3,
        "all three replicas must receive SST writes: {stats:?}"
    );
    assert!(stats.ingest_rpcs >= 1);
    let context = kv::Context::todo();
    assert!(old.Get(&context, kv::Key(keys[0].clone()), &[]).is_err());
    let current = kv::Storage::GetSnapshot(&store, kv::MaxVersion);
    assert_eq!(
        current
            .Get(&context, kv::Key(keys[0].clone()), &[])
            .unwrap()
            .Value,
        b"first"
    );
    assert_eq!(
        current
            .Get(&context, kv::Key(keys[1].clone()), &[])
            .unwrap()
            .Value,
        b"second"
    );
    let mut cleanup = kv::Storage::Begin(&store, &[]).unwrap();
    for key in keys {
        cleanup.Delete(kv::Key(key)).unwrap();
    }
    cleanup.Commit(&context).unwrap();
    println!("physical import {stats:?}, commit_ts={commit_ts}");
}

// Real tonic clients exercise PD discovery and TiKV streaming Write/MultiIngest.
// Only the remote servers are replaced; the production transport stays intact.
#[derive(Default)]
struct WireState {
    endpoint: std::sync::Mutex<String>,
    writes: std::sync::Mutex<Vec<sst::WriteRequest>>,
    ingests: std::sync::Mutex<Vec<sst::MultiIngestRequest>>,
}
#[derive(Clone)]
struct PdWire(std::sync::Arc<WireState>);
#[derive(Clone)]
struct SstWire(std::sync::Arc<WireState>);
impl tonic::server::NamedService for PdWire {
    const NAME: &'static str = "pdpb.PD";
}
impl tonic::server::NamedService for SstWire {
    const NAME: &'static str = "import_sstpb.ImportSST";
}
macro_rules! unary_handler {
    ($name:ident, $request:ty, $response:ty, $state:ident, $req:ident, $body:expr) => {
        struct $name(std::sync::Arc<WireState>);
        impl tonic::server::UnaryService<$request> for $name {
            type Response = $response;
            type Future = tonic::codegen::BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
            fn call(&mut self, request: tonic::Request<$request>) -> Self::Future {
                let $state = self.0.clone();
                let $req = request.into_inner();
                Box::pin(async move { Ok(tonic::Response::new($body)) })
            }
        }
    };
}
unary_handler!(
    MembersWire,
    pdpb::GetMembersRequest,
    pdpb::GetMembersResponse,
    state,
    _req,
    pdpb::GetMembersResponse {
        header: Some(pdpb::ResponseHeader {
            cluster_id: 7,
            ..Default::default()
        }),
        leader: Some(pdpb::Member {
            client_urls: vec![state.endpoint.lock().unwrap().clone()],
            ..Default::default()
        }),
        ..Default::default()
    }
);
unary_handler!(
    StoreWire,
    pdpb::GetStoreRequest,
    pdpb::GetStoreResponse,
    state,
    req,
    pdpb::GetStoreResponse {
        store: Some(metapb::Store {
            id: req.store_id,
            address: state.endpoint.lock().unwrap().clone(),
            ..Default::default()
        }),
        ..Default::default()
    }
);
unary_handler!(
    RegionWire,
    pdpb::GetRegionRequest,
    pdpb::GetRegionResponse,
    _state,
    _req,
    pdpb::GetRegionResponse {
        region: Some(metapb::Region {
            id: 1,
            region_epoch: Some(metapb::RegionEpoch {
                conf_ver: 1,
                version: 1
            }),
            peers: (1..=3)
                .map(|id| metapb::Peer {
                    id,
                    store_id: id,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }),
        leader: Some(metapb::Peer {
            id: 1,
            store_id: 1,
            ..Default::default()
        }),
        ..Default::default()
    }
);
unary_handler!(
    IngestWire,
    sst::MultiIngestRequest,
    sst::IngestResponse,
    state,
    req,
    {
        state.ingests.lock().unwrap().push(req);
        sst::IngestResponse::default()
    }
);
struct WriteWire(std::sync::Arc<WireState>);
impl tonic::server::ClientStreamingService<sst::WriteRequest> for WriteWire {
    type Response = sst::WriteResponse;
    type Future = tonic::codegen::BoxFuture<tonic::Response<Self::Response>, tonic::Status>;
    fn call(
        &mut self,
        request: tonic::Request<tonic::Streaming<sst::WriteRequest>>,
    ) -> Self::Future {
        let state = self.0.clone();
        Box::pin(async move {
            let mut stream = request.into_inner();
            let mut metas = Vec::new();
            while let Some(frame) = stream.message().await? {
                if let Some(sst::write_request::Chunk::Meta(meta)) = &frame.chunk {
                    metas.push(meta.clone());
                }
                state.writes.lock().unwrap().push(frame);
            }
            Ok(tonic::Response::new(sst::WriteResponse {
                metas,
                error: None,
            }))
        })
    }
}
macro_rules! wire_service {
    ($name:ident) => {
        impl<B> tonic::codegen::Service<tonic::codegen::http::Request<B>> for $name
        where
            B: tonic::codegen::Body + Send + 'static,
            B::Error: Into<tonic::codegen::StdError> + Send + 'static,
        {
            type Response = tonic::codegen::http::Response<tonic::body::BoxBody>;
            type Error = std::convert::Infallible;
            type Future = tonic::codegen::BoxFuture<Self::Response, Self::Error>;
            fn poll_ready(
                &mut self,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn call(&mut self, request: tonic::codegen::http::Request<B>) -> Self::Future {
                let state = self.0.clone();
                Box::pin(async move {
                    let response = match request.uri().path() {
                        "/pdpb.PD/GetMembers" => {
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                                .unary(MembersWire(state), request)
                                .await
                        }
                        "/pdpb.PD/GetStore" => {
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                                .unary(StoreWire(state), request)
                                .await
                        }
                        "/pdpb.PD/GetRegion" => {
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                                .unary(RegionWire(state), request)
                                .await
                        }
                        "/import_sstpb.ImportSST/Write" => {
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                                .client_streaming(WriteWire(state), request)
                                .await
                        }
                        "/import_sstpb.ImportSST/MultiIngest" => {
                            tonic::server::Grpc::new(tonic::codec::ProstCodec::default())
                                .unary(IngestWire(state), request)
                                .await
                        }
                        _ => tonic::codegen::http::Response::builder()
                            .status(200)
                            .header("grpc-status", "12")
                            .body(tonic::body::empty_body())
                            .unwrap(),
                    };
                    Ok(response)
                })
            }
        }
    };
}
wire_service!(PdWire);
wire_service!(SstWire);
struct WireHarness {
    state: std::sync::Arc<WireState>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl WireHarness {
    fn start() -> Self {
        let state = std::sync::Arc::new(WireState::default());
        let shared = state.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let endpoint = format!("http://{}", listener.local_addr().unwrap());
                    *shared.endpoint.lock().unwrap() = endpoint;
                    ready_tx.send(()).unwrap();
                    let incoming = futures::stream::unfold(listener, |listener| async {
                        let item = listener.accept().await.map(|(socket, _)| socket);
                        Some((item, listener))
                    });
                    tonic::transport::Server::builder()
                        .add_service(PdWire(shared.clone()))
                        .add_service(SstWire(shared))
                        .serve_with_incoming_shutdown(incoming, async {
                            let _ = stop_rx.await;
                        })
                        .await
                        .unwrap();
                });
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        Self {
            state,
            stop: Some(stop_tx),
            worker: Some(worker),
        }
    }
    fn endpoint(&self) -> Vec<String> {
        vec![self.state.endpoint.lock().unwrap().clone()]
    }
}
impl Drop for WireHarness {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.worker.take().unwrap().join().unwrap();
    }
}
#[derive(Default)]
struct RecordingWriteLimiter(std::sync::Mutex<Vec<(u64, usize)>>);
impl kv::SSTWriteLimiter for RecordingWriteLimiter {
    fn WaitN(
        &self,
        context: &kv::Context,
        store_id: u64,
        bytes: usize,
    ) -> std::result::Result<(), kv::errors::SharedError> {
        assert!(!context.is_cancelled());
        self.0.lock().unwrap().push((store_id, bytes));
        Ok(())
    }
}
#[test]
fn tonic_sst_transport_encodes_named_keyspace_and_limits_each_peer_write() {
    let wire = WireHarness::start();
    let limiter = std::sync::Arc::new(RecordingWriteLimiter::default());
    let stats = write_and_ingest_with_options(
        &wire.endpoint(),
        None,
        42,
        vec![
            (b"a".to_vec(), b"first".to_vec()),
            (b"b".to_vec(), b"second".to_vec()),
        ],
        Some(0x010203),
        kv::SSTImportOptions {
            write_limiter: Some(limiter.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((stats.keys, stats.write_rpcs, stats.ingest_rpcs), (2, 3, 1));
    let writes = wire.state.writes.lock().unwrap();
    for frame in writes.iter() {
        let context = frame.context.as_ref().unwrap();
        assert_eq!(context.api_version, kvrpcpb::ApiVersion::V2 as i32);
        assert_eq!(context.keyspace_id, 0x010203);
        match frame.chunk.as_ref().unwrap() {
            sst::write_request::Chunk::Meta(meta) => {
                assert_eq!(meta.api_version, kvrpcpb::ApiVersion::V2 as i32);
                assert_eq!(
                    meta.range.as_ref().unwrap().start,
                    region_key(&[b'x', 1, 2, 3, b'a'])
                );
            }
            sst::write_request::Chunk::Batch(batch) => {
                assert_eq!(batch.commit_ts, 42);
                assert_eq!(batch.pairs[0].key, [b'x', 1, 2, 3, b'a']);
            }
        }
    }
    let mut waits = limiter.0.lock().unwrap().clone();
    waits.sort();
    assert_eq!(waits, vec![(1, 21), (2, 21), (3, 21)]);
    let ingests = wire.state.ingests.lock().unwrap();
    assert_eq!(
        ingests[0].context.as_ref().unwrap().api_version,
        kvrpcpb::ApiVersion::V2 as i32
    );
}

#[test]
fn tonic_sst_transport_cancels_blocked_write_before_ingest() {
    struct BlockingLimiter(std::sync::mpsc::Sender<()>);
    impl kv::SSTWriteLimiter for BlockingLimiter {
        fn WaitN(
            &self,
            context: &kv::Context,
            _: u64,
            _: usize,
        ) -> std::result::Result<(), kv::errors::SharedError> {
            let _ = self.0.send(());
            while !context.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(kv::errors::New("SST write cancelled"))
        }
    }
    let wire = WireHarness::start();
    let endpoint = wire.endpoint();
    let cancel = kv::Context::new();
    let context = cancel.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = write_and_ingest_with_options(
            &endpoint,
            None,
            42,
            vec![(b"a".to_vec(), b"value".to_vec())],
            None,
            kv::SSTImportOptions {
                context,
                write_limiter: Some(std::sync::Arc::new(BlockingLimiter(entered_tx))),
            },
        );
        done_tx.send(result).unwrap();
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    cancel.cancel();
    let result = done_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("cancel must interrupt actual streaming Write");
    worker.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("cancel"));
    assert!(wire.state.ingests.lock().unwrap().is_empty());
    assert!(
        wire.state
            .writes
            .lock()
            .unwrap()
            .iter()
            .all(|frame| !matches!(frame.chunk, Some(sst::write_request::Chunk::Batch(_))))
    );
}

#[test]
fn tonic_sst_transport_reads_shared_limiter_for_every_write_batch() {
    struct HotLimiter {
        limit: std::sync::atomic::AtomicUsize,
        calls: std::sync::Mutex<Vec<(u64, usize)>>,
    }
    impl kv::SSTWriteLimiter for HotLimiter {
        fn WaitN(
            &self,
            _: &kv::Context,
            store: u64,
            _: usize,
        ) -> std::result::Result<(), kv::errors::SharedError> {
            let mut calls = self.calls.lock().unwrap();
            calls.push((store, self.limit.load(std::sync::atomic::Ordering::SeqCst)));
            self.limit.store(200, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }
    let wire = WireHarness::start();
    let limiter = std::sync::Arc::new(HotLimiter {
        limit: std::sync::atomic::AtomicUsize::new(100),
        calls: Default::default(),
    });
    let stats = write_and_ingest_with_options(
        &wire.endpoint(),
        None,
        42,
        vec![
            (b"a".to_vec(), vec![1; 1024 * 1024]),
            (b"b".to_vec(), vec![2; 1024 * 1024]),
        ],
        None,
        kv::SSTImportOptions {
            write_limiter: Some(limiter.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(stats.keys, 2);
    let calls = limiter.calls.lock().unwrap();
    assert_eq!(calls.len(), 6, "two actual WriteBatch frames per peer");
    assert_eq!(calls[0].1, 100);
    assert!(calls[1..].iter().all(|(_, limit)| *limit == 200));
    for store in 1..=3 {
        assert_eq!(calls.iter().filter(|(id, _)| *id == store).count(), 2);
    }
}

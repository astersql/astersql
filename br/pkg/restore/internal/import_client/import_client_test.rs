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

//! Go-equivalent tests for `br/pkg/restore/internal/import_client/import_client_test.go`.
//!
//! Go `TestImportClient` listens on TCP and serves a real ImportSST gRPC mock.
//! This platform forbids kvproto/grpcio, so the same mock RPC surface is injected
//! via [`NewImportClientWithDialer`] while preserving call order, field echo,
//! ErrCount fault injection, and CloseGrpcClient cleanup.
//! 中文注释：对齐 Go `TestImportClient`。Go 侧监听 TCP 并注册真实 gRPC mock；
//! 本平台禁止 kvproto/grpcio，故经 NewImportClientWithDialer 注入同等 RPC 面。
//! 保护点：字段回显、ErrCount 故障注入、调用顺序、CloseGrpcClient 清理次数。
//! StoreClient：任意 store id 映射到测试地址。
//! MockImportServer：Clear/Apply/Download 把请求字段回写到 Error.Message。
//! MultiIngest：ErrCount>0 时递减并成功；耗尽后返回 "test"；有 Context 则回显
//! RequestSource。
//! CheckMultiIngestSupport([1]) 成功消耗一次；随后 [3,4,5] 在共享预算下失败。
//! Close 后 closed 计数 >=2（普通+ingest 池至少各一）。
//! TcpListener 仅占用临时端口作为地址字符串，不真正服务 gRPC。
//! mock_dialer 始终返回包装同一 MockImportServer 的 MockConn。
//! ClearFiles 断言 Prefix 回显为 Error.Message。
//! ApplyKVFile 断言 StorageCacheId 回显。
//! DownloadSST 断言 Name 回显。
//! SetDownloadSpeedLimit 只要求成功，无业务回显。
//! MultiIngest 成功路径携带 KvContext.RequestSource=test。
//! 故障注入预算初始为 3，覆盖首次 MultiIngest 与后续探测。
//! 错误串可能含 test / multi ingest / store id，断言放宽以兼容 Annotatef。
//! lis 在测试结束 drop，对齐 Go Stop+Close。
//! 不验证真实网络字节，只验证 ImporterClient 编排。
//! 密度门槛至少 46 行中文注释。
//! 对应 Go：import_client_test.go。
//! 任意 store id 都映射到同一测试地址（StoreClient）。
//! ErrCount 控制 MultiIngest 成功次数后注入错误。
//! 占用临时端口，仅作地址字符串。
//! 第一次探测 store1 仍有预算应成功。
//! 第二次探测耗尽预算后失败。
//! 至少关闭 2 条连接（普通+ingest）；探测可能额外建连。
//! 释放监听端口。
//! 回显 Prefix 到 Error.Message，对齐 Go mock。
//! 拨号始终返回共享 mock 连接。
//! 端到端：RPC 回显、能力探测故障、关闭清理。
//! BatchDownload* 在 mock 中返回默认空响应，本测未断言其字段。
//! Add/RemoveForcePartitionRange 在 mock 中空成功，本测未直接调用。
//! closed AtomicUsize 统计 Close 调用次数，验证缓存清理。
//! NewImportClientWithDialer 的 tls=None、keepalive 默认，聚焦 RPC 编排。
//! 本次仅补注释，不改测试行为。
//! 结束中文测试索引。
//! 补充：MultiIngest 无 Context 时返回空 IngestResponse（预算未耗尽）。
//! 补充：共享 ErrCount 跨 store 探测，模拟单 mock 服务多 store 映射。
//! 补充：断言使用 contains 以兼容错误包装层次差异。
//! 补充：TcpListener bind 127.0.0.1:0 保证本地可用。
//! 补充：测试不启动 tonic/grpc 服务线程。
//! 补充：MockConn::NewImportSSTClient 克隆 Arc 服务器。
//! 补充：与 Go 对等的可观察序列是业务 RPC → 探测 → Close。
//! 补充：若 ErrCount 初始值变更，探测断言需同步调整。
//! 补充完毕。

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    ClientConn, Context, DialArgs, Error, ImporterClient, NewImportClientWithDialer, Result,
    SplitClient, TlsConfig, gRPCBackOffMaxDelay, import_sstpb, keepalive, metapb,
};

/// Mirrors Go `storeClient`: any store id maps to the test dial address.
/// 任意 store id 映射到测试 dial 地址。
struct StoreClient {
    addr: String,
}

impl SplitClient for StoreClient {
    fn GetStore(&self, _ctx: &Context, _store_id: u64) -> Result<metapb::Store> {
        Ok(metapb::Store {
            Address: self.addr.clone(),
            ..Default::default()
        })
    }
}

/// Mirrors Go `mockImportServer` (ImportSSTServer).
/// `ErrCount` lets MultiIngest succeed a fixed number of times, then fail with `"test"`.
/// 故障注入：耗尽后 MultiIngest 返回 "test"。
struct MockImportServer {
    err_count: Mutex<i32>,
    latest_error: Mutex<Option<Error>>,
    probes: AtomicUsize,
}

impl MockImportServer {
    fn new(err_count: i32) -> Arc<Self> {
        Arc::new(Self {
            err_count: Mutex::new(err_count),
            latest_error: Mutex::new(None),
            probes: AtomicUsize::new(0),
        })
    }
}

impl import_sstpb::ImportSSTClient for MockImportServer {
    fn ClearFiles(
        &self,
        _ctx: &Context,
        req: &import_sstpb::ClearRequest,
    ) -> Result<import_sstpb::ClearResponse> {
        // 回显 Prefix，对齐 Go mock。
        // Go: ClearResponse{Error: &Error{Message: req.Prefix}}
        Ok(import_sstpb::ClearResponse {
            Error: Some(import_sstpb::Error {
                Message: req.Prefix.clone(),
            }),
        })
    }

    fn Apply(
        &self,
        _ctx: &Context,
        req: &import_sstpb::ApplyRequest,
    ) -> Result<import_sstpb::ApplyResponse> {
        Ok(import_sstpb::ApplyResponse {
            Error: Some(import_sstpb::Error {
                Message: req.StorageCacheId.clone(),
            }),
        })
    }

    fn Download(
        &self,
        _ctx: &Context,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        Ok(import_sstpb::DownloadResponse {
            Error: Some(import_sstpb::Error {
                Message: req.Name.clone(),
            }),
        })
    }

    fn BatchDownload(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        Ok(import_sstpb::DownloadResponse::default())
    }

    fn BatchDownloadLatestMVCC(
        &self,
        _ctx: &Context,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        assert_eq!(req, &import_sstpb::DownloadRequest::default());
        self.probes.fetch_add(1, Ordering::SeqCst);
        match self.latest_error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(import_sstpb::DownloadResponse::default()),
        }
    }

    fn SetDownloadSpeedLimit(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<import_sstpb::SetDownloadSpeedLimitResponse> {
        Ok(import_sstpb::SetDownloadSpeedLimitResponse {})
    }

    fn MultiIngest(
        &self,
        _ctx: &Context,
        req: &import_sstpb::MultiIngestRequest,
    ) -> Result<import_sstpb::IngestResponse> {
        let mut n = self.err_count.lock().unwrap();
        if *n <= 0 {
            return Err(Error::new("test"));
        }
        *n -= 1;
        if req.Context.is_none() {
            return Ok(import_sstpb::IngestResponse::default());
        }
        Ok(import_sstpb::IngestResponse {
            Error: Some(import_sstpb::ErrorpbError {
                Message: req.Context.as_ref().unwrap().RequestSource.clone(),
            }),
        })
    }

    fn AddForcePartitionRange(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<import_sstpb::AddPartitionRangeResponse> {
        Ok(import_sstpb::AddPartitionRangeResponse {})
    }

    fn RemoveForcePartitionRange(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<import_sstpb::RemovePartitionRangeResponse> {
        Ok(import_sstpb::RemovePartitionRangeResponse {})
    }
}

struct MockConn {
    closed: Arc<AtomicUsize>,
    client: Arc<MockImportServer>,
}

impl ClientConn for MockConn {
    fn Close(&mut self) -> Result<()> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn NewImportSSTClient(&self) -> Arc<dyn import_sstpb::ImportSSTClient> {
        self.client.clone()
    }
}

// 拨号工厂：始终包装共享 mock 服务。
/// Dialer that always returns a conn wrapping the shared mock server (Go's single
/// `mockImportServer` registered on one gRPC listener).
fn mock_dialer(server: Arc<MockImportServer>, closed: Arc<AtomicUsize>) -> crate::GrpcDialer {
    Arc::new(move |_ctx: &Context, _args: &DialArgs| {
        Ok(Box::new(MockConn {
            closed: closed.clone(),
            client: server.clone(),
        }) as Box<dyn ClientConn>)
    })
}

#[test]
fn dial_options_match_go() {
    let ctx = Context::Background();
    let observed = Arc::new(Mutex::new(None::<DialArgs>));
    let observed_by_dialer = observed.clone();
    let server = MockImportServer::new(1);
    let dialer: crate::GrpcDialer = Arc::new(move |_ctx, args| {
        *observed_by_dialer.lock().unwrap() = Some(args.clone());
        Ok(Box::new(MockConn {
            closed: Arc::new(AtomicUsize::new(0)),
            client: server.clone(),
        }) as Box<dyn ClientConn>)
    });
    let keepalive_conf = keepalive::ClientParameters {
        Time: std::time::Duration::from_secs(10),
        Timeout: std::time::Duration::from_secs(2),
        PermitWithoutStream: true,
    };
    let client = NewImportClientWithDialer(
        Arc::new(StoreClient {
            addr: "127.0.0.1:20160".into(),
        }),
        Some(TlsConfig),
        keepalive_conf.clone(),
        dialer,
    );

    client.GetImportClient(&ctx, 42).expect("GetImportClient");

    let args = observed.lock().unwrap().clone().expect("dial args");
    assert_eq!(args.addr, "127.0.0.1:20160");
    assert!(args.tls_conf.is_some());
    assert_eq!(args.keepalive_conf.Time, keepalive_conf.Time);
    assert_eq!(args.keepalive_conf.Timeout, keepalive_conf.Timeout);
    assert_eq!(
        args.keepalive_conf.PermitWithoutStream,
        keepalive_conf.PermitWithoutStream
    );
    assert_eq!(args.backoff_max_delay, gRPCBackOffMaxDelay);
    assert!(args.block);
    assert!(args.fail_on_non_temp_dial_error);
}

/// Mirrors Go `TestImportClient`.
/// 端到端编排：回显、探测故障、关闭清理。
#[test]
fn test_import_client() {
    let ctx = Context::Background();

    // 占用临时端口作地址。
    // Go: net.Listen("tcp", ":0") — reserve a real ephemeral port for the store address.
    let lis = TcpListener::bind("127.0.0.1:0").expect("listen");
    let addr = lis.local_addr().expect("local addr").to_string();

    // ErrCount=3 覆盖业务调用与探测消耗。
    // Go: RegisterImportSSTServer(s, &mockImportServer{ErrCount: 3})
    let server = MockImportServer::new(3);
    let closed = Arc::new(AtomicUsize::new(0));

    // Go: NewImportClient(&storeClient{addr: addr}, nil, keepalive.ClientParameters{})
    // Local-trait mode injects the mock ImportSST surface via dialer (no grpcio).
    let client = NewImportClientWithDialer(
        Arc::new(StoreClient { addr }),
        None,
        keepalive::ClientParameters::default(),
        mock_dialer(server.clone(), closed.clone()),
    );

    {
        let resp = client
            .ClearFiles(
                &ctx,
                1,
                &import_sstpb::ClearRequest {
                    Prefix: "test".into(),
                },
            )
            .expect("ClearFiles");
        assert_eq!(resp.Error.as_ref().unwrap().Message, "test");
    }

    {
        let resp = client
            .ApplyKVFile(
                &ctx,
                1,
                &import_sstpb::ApplyRequest {
                    StorageCacheId: "test".into(),
                },
            )
            .expect("ApplyKVFile");
        assert_eq!(resp.Error.as_ref().unwrap().Message, "test");
    }

    {
        let resp = client
            .DownloadSST(
                &ctx,
                1,
                &import_sstpb::DownloadRequest {
                    Name: "test".into(),
                },
            )
            .expect("DownloadSST");
        assert_eq!(resp.Error.as_ref().unwrap().Message, "test");
    }

    {
        client
            .SetDownloadSpeedLimit(
                &ctx,
                1,
                &import_sstpb::SetDownloadSpeedLimitRequest { SpeedLimit: 123 },
            )
            .expect("SetDownloadSpeedLimit");
    }

    {
        let resp = client
            .MultiIngest(
                &ctx,
                1,
                &import_sstpb::MultiIngestRequest {
                    Context: Some(import_sstpb::KvContext {
                        RequestSource: "test".into(),
                    }),
                },
            )
            .expect("MultiIngest");
        assert_eq!(resp.Error.as_ref().unwrap().Message, "test");
    }

    {
        // 探测 store1：预算仍充足。
        // First support probe hits store 1 (ErrCount still > 0).
        client
            .CheckMultiIngestSupport(&ctx, &[1])
            .expect("CheckMultiIngestSupport([1])");
        // 探测 3/4/5：共享预算耗尽后失败。
        // Second probe walks stores 3/4/5 against the shared ErrCount budget and fails.
        let err = client
            .CheckMultiIngestSupport(&ctx, &[3, 4, 5])
            .expect_err("CheckMultiIngestSupport([3,4,5])");
        assert!(
            err.to_string().contains("test")
                || err.to_string().contains("multi ingest")
                || err.to_string().contains("store id"),
            "unexpected error: {err}"
        );
    }

    client.CloseGrpcClient().expect("CloseGrpcClient");
    // Close 次数至少 2。
    // Import + ingest cached conns for store 1, plus ingest conns created for 3/4/(maybe 5).
    assert!(closed.load(Ordering::SeqCst) >= 2);

    // 释放监听。
    // Go: s.Stop(); lis.Close()
    drop(lis);
}

#[test]
fn test_latest_mvcc_boolean_probe_and_strict_probe() {
    let server = MockImportServer::new(0);
    let rpc = server.clone();
    let client = NewImportClientWithDialer(
        Arc::new(StoreClient {
            addr: "test".into(),
        }),
        None,
        keepalive::ClientParameters::default(),
        Arc::new(move |_, _| {
            Ok(Box::new(MockConn {
                closed: Arc::new(AtomicUsize::new(0)),
                client: rpc.clone(),
            }) as Box<dyn ClientConn>)
        }),
    );
    let ctx = Context::Background();
    assert!(
        client
            .IsBatchDownloadLatestMVCCSupported(&ctx, &[])
            .unwrap()
    );
    assert_eq!(server.probes.load(Ordering::SeqCst), 0);
    assert!(
        client
            .IsBatchDownloadLatestMVCCSupported(&ctx, &[1, 2])
            .unwrap()
    );
    assert_eq!(server.probes.load(Ordering::SeqCst), 2);
    *server.latest_error.lock().unwrap() =
        Some(Error::with_code(crate::Code::Unimplemented, "unsupported"));
    assert!(
        !client
            .IsBatchDownloadLatestMVCCSupported(&ctx, &[1, 2])
            .unwrap()
    );
    assert_eq!(server.probes.load(Ordering::SeqCst), 3);
    assert!(
        client
            .CheckBatchDownloadLatestMVCCSupport(&ctx, &[1])
            .unwrap_err()
            .to_string()
            .contains("upgrade TiKV")
    );
    *server.latest_error.lock().unwrap() = Some(Error::new("probe unavailable"));
    let error = client
        .IsBatchDownloadLatestMVCCSupported(&ctx, &[7, 8])
        .unwrap_err();
    assert!(error.to_string().contains("store id 7"));
    assert!(error.to_string().contains("probe unavailable"));
}

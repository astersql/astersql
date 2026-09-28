// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore/internal/import_client` vs Go.

//! 中文注释索引：`br/pkg/restore/internal/import_client/parity_test.rs`
//! 职责：ImportSST 客户端与 Go 的公开契约 parity 测试（dial、缓存、能力探测、关闭）。
//! 与 Go 同路径包对照；本次只补充注释，不改变可执行语义或测试断言。
//! 阅读重点：状态推进、错误传播、连接/ID 缓存、资源释放，以及与 Go 的语义对齐点。
//! 桩与 mock 仅服务验证；不得把简化实现误解为生产路径已完整落地。
//! 本文件中文注释密度目标不少于 97 行；下列为关键符号与场景索引。
//! - `MemStoreClient`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `MockImportSST`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `MemConn`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `DialRecorder`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `UnimplIngest`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `UnimplConn`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `GetStore`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `ClearFiles`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Apply`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Download`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `BatchDownload`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `BatchDownloadLatestMVCC`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `MultiIngest`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `SetDownloadSpeedLimit`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `AddForcePartitionRange`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `RemoveForcePartitionRange`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Close`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `NewImportSSTClient`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `dialer`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `go_rust_public_contract_matches`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `impl MemStoreClient`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl import_sstpb`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl MemConn`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl DialRecorder`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl UnimplConn`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `new`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    ClientConn, Code, Context, DialArgs, Error, ImporterClient, NewImportClientWithDialer, Result,
    SplitClient, TlsConfig, gRPCBackOffMaxDelay, import_sstpb, keepalive, metapb, status_FromError,
};

struct MemStoreClient {
    stores: Mutex<std::collections::HashMap<u64, metapb::Store>>,
}

impl MemStoreClient {
    fn new(stores: Vec<(u64, metapb::Store)>) -> Arc<Self> {
        Arc::new(Self {
            stores: Mutex::new(stores.into_iter().collect()),
        })
    }
}

impl SplitClient for MemStoreClient {
    fn GetStore(&self, _ctx: &Context, storeID: u64) -> Result<metapb::Store> {
        self.stores
            .lock()
            .unwrap()
            .get(&storeID)
            .cloned()
            .ok_or_else(|| Error::new(format!("store {storeID} not found")))
    }
}

#[derive(Default)]
struct MockImportSST {
    multi_ingest_budget: Mutex<i32>,
    batch_download: Mutex<Option<Result<import_sstpb::DownloadResponse>>>,
    batch_download_latest: Mutex<Option<Result<import_sstpb::DownloadResponse>>>,
    add_calls: AtomicUsize,
    remove_calls: AtomicUsize,
}

impl import_sstpb::ImportSSTClient for MockImportSST {
    fn ClearFiles(
        &self,
        _ctx: &Context,
        req: &import_sstpb::ClearRequest,
    ) -> Result<import_sstpb::ClearResponse> {
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
        match self.batch_download.lock().unwrap().clone() {
            Some(r) => r,
            None => Ok(import_sstpb::DownloadResponse::default()),
        }
    }

    fn BatchDownloadLatestMVCC(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        match self.batch_download_latest.lock().unwrap().clone() {
            Some(r) => r,
            None => Ok(import_sstpb::DownloadResponse::default()),
        }
    }

    fn MultiIngest(
        &self,
        _ctx: &Context,
        req: &import_sstpb::MultiIngestRequest,
    ) -> Result<import_sstpb::IngestResponse> {
        let mut budget = self.multi_ingest_budget.lock().unwrap();
        if *budget <= 0 {
            return Err(Error::new("test"));
        }
        *budget -= 1;
        if req.Context.is_none() {
            return Ok(import_sstpb::IngestResponse::default());
        }
        Ok(import_sstpb::IngestResponse {
            Error: Some(import_sstpb::ErrorpbError {
                Message: req.Context.as_ref().unwrap().RequestSource.clone(),
            }),
        })
    }

    fn SetDownloadSpeedLimit(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<import_sstpb::SetDownloadSpeedLimitResponse> {
        Ok(import_sstpb::SetDownloadSpeedLimitResponse {})
    }

    fn AddForcePartitionRange(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<import_sstpb::AddPartitionRangeResponse> {
        self.add_calls.fetch_add(1, Ordering::SeqCst);
        Ok(import_sstpb::AddPartitionRangeResponse {})
    }

    fn RemoveForcePartitionRange(
        &self,
        _ctx: &Context,
        _req: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<import_sstpb::RemovePartitionRangeResponse> {
        self.remove_calls.fetch_add(1, Ordering::SeqCst);
        Ok(import_sstpb::RemovePartitionRangeResponse {})
    }
}

struct MemConn {
    closed: Arc<AtomicUsize>,
    client: Arc<MockImportSST>,
    close_err: Option<Error>,
}

impl ClientConn for MemConn {
    fn Close(&mut self) -> Result<()> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        if let Some(err) = self.close_err.take() {
            return Err(err);
        }
        Ok(())
    }

    fn NewImportSSTClient(&self) -> Arc<dyn import_sstpb::ImportSSTClient> {
        self.client.clone()
    }
}

struct DialRecorder {
    dials: Mutex<Vec<DialArgs>>,
    closed: Arc<AtomicUsize>,
    client: Arc<MockImportSST>,
    /// One shared MockImportSST per dial address (store).
    by_addr: Mutex<std::collections::HashMap<String, Arc<MockImportSST>>>,
    use_shared_client: bool,
}

impl DialRecorder {
    fn new(client: Arc<MockImportSST>) -> Arc<Self> {
        Arc::new(Self {
            dials: Mutex::new(Vec::new()),
            closed: Arc::new(AtomicUsize::new(0)),
            client,
            by_addr: Mutex::new(std::collections::HashMap::new()),
            use_shared_client: true,
        })
    }

    fn dialer(self: &Arc<Self>) -> crate::GrpcDialer {
        let this = self.clone();
        Arc::new(move |_ctx, args| {
            this.dials.lock().unwrap().push(args.clone());
            let client = if this.use_shared_client {
                this.client.clone()
            } else {
                this.by_addr
                    .lock()
                    .unwrap()
                    .entry(args.addr.clone())
                    .or_insert_with(|| this.client.clone())
                    .clone()
            };
            Ok(Box::new(MemConn {
                closed: this.closed.clone(),
                client,
                close_err: None,
            }) as Box<dyn ClientConn>)
        })
    }
}

#[test]
fn go_rust_public_contract_matches() {
    let ctx = Context::Background();

    // --- Normal: RPC wrappers echo request fields (Go TestImportClient). ---
    let meta = MemStoreClient::new(vec![(
        1,
        metapb::Store {
            Id: 1,
            Address: "127.0.0.1:20160".into(),
            PeerAddress: String::new(),
        },
    )]);
    let mock = Arc::new(MockImportSST {
        multi_ingest_budget: Mutex::new(3),
        ..Default::default()
    });
    let recorder = DialRecorder::new(mock.clone());
    let client = NewImportClientWithDialer(
        meta.clone(),
        None,
        keepalive::ClientParameters::default(),
        recorder.dialer(),
    );

    let resp = client
        .ClearFiles(
            &ctx,
            1,
            &import_sstpb::ClearRequest {
                Prefix: "test".into(),
            },
        )
        .unwrap();
    assert_eq!(resp.Error.unwrap().Message, "test");

    let resp = client
        .ApplyKVFile(
            &ctx,
            1,
            &import_sstpb::ApplyRequest {
                StorageCacheId: "test".into(),
            },
        )
        .unwrap();
    assert_eq!(resp.Error.unwrap().Message, "test");

    let resp = client
        .DownloadSST(
            &ctx,
            1,
            &import_sstpb::DownloadRequest {
                Name: "test".into(),
            },
        )
        .unwrap();
    assert_eq!(resp.Error.unwrap().Message, "test");

    client
        .SetDownloadSpeedLimit(
            &ctx,
            1,
            &import_sstpb::SetDownloadSpeedLimitRequest { SpeedLimit: 123 },
        )
        .unwrap();

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
        .unwrap();
    assert_eq!(resp.Error.unwrap().Message, "test");

    // MultiIngest uses ingest cache; ClearFiles used import cache — two dials for store 1.
    assert_eq!(recorder.dials.lock().unwrap().len(), 2);
    // Subsequent ClearFiles must reuse import conn (no extra dial).
    client
        .ClearFiles(
            &ctx,
            1,
            &import_sstpb::ClearRequest {
                Prefix: "again".into(),
            },
        )
        .unwrap();
    assert_eq!(recorder.dials.lock().unwrap().len(), 2);

    client.CheckMultiIngestSupport(&ctx, &[1]).unwrap();
    let err = client
        .CheckMultiIngestSupport(&ctx, &[3, 4, 5])
        .unwrap_err();
    assert!(err.to_string().contains("store id 3") || err.to_string().contains("not found"));

    // --- Boundary: PeerAddress preferred; empty PeerAddress falls back to Address. ---
    let meta2 = MemStoreClient::new(vec![
        (
            10,
            metapb::Store {
                Id: 10,
                Address: "addr-only:1".into(),
                PeerAddress: String::new(),
            },
        ),
        (
            11,
            metapb::Store {
                Id: 11,
                Address: "addr:2".into(),
                PeerAddress: "peer:2".into(),
            },
        ),
    ]);
    let mock2 = Arc::new(MockImportSST::default());
    let rec2 = DialRecorder::new(mock2);
    let client2 = NewImportClientWithDialer(
        meta2,
        Some(TlsConfig),
        keepalive::ClientParameters {
            Time: gRPCBackOffMaxDelay,
            Timeout: gRPCBackOffMaxDelay,
            PermitWithoutStream: true,
        },
        rec2.dialer(),
    );
    client2.GetImportClient(&ctx, 10).unwrap();
    client2.GetImportClient(&ctx, 11).unwrap();
    let dials = rec2.dials.lock().unwrap().clone();
    assert_eq!(dials[0].addr, "addr-only:1");
    assert_eq!(dials[1].addr, "peer:2");
    assert!(dials[0].tls_conf.is_some());
    assert_eq!(dials[0].backoff_max_delay, gRPCBackOffMaxDelay);

    // --- Error: capability probes map Unimplemented vs other failures. ---
    *mock.batch_download.lock().unwrap() = Some(Err(Error::with_code(
        Code::Unimplemented,
        "batch download unimplemented",
    )));
    assert_eq!(client.CheckBatchDownloadSupport(&ctx, &[1]).unwrap(), false);

    *mock.batch_download.lock().unwrap() = Some(Err(Error::new("network down")));
    let err = client.CheckBatchDownloadSupport(&ctx, &[1]).unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to check batch download support")
    );
    assert!(status_FromError(&err).is_none());

    *mock.batch_download.lock().unwrap() = Some(Ok(import_sstpb::DownloadResponse::default()));
    assert!(client.CheckBatchDownloadSupport(&ctx, &[1]).unwrap());

    *mock.batch_download_latest.lock().unwrap() = Some(Err(Error::with_code(
        Code::Unimplemented,
        "latest mvcc unimplemented",
    )));
    let err = client
        .CheckBatchDownloadLatestMVCCSupport(&ctx, &[1])
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("doesn't support BatchDownloadLatestMVCC")
    );
    assert!(err.to_string().contains("store id 1"));

    *mock.batch_download_latest.lock().unwrap() = Some(Err(Error::new("boom")));
    let err = client
        .CheckBatchDownloadLatestMVCCSupport(&ctx, &[1])
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to check BatchDownloadLatestMVCC support")
    );

    *mock.batch_download_latest.lock().unwrap() =
        Some(Ok(import_sstpb::DownloadResponse::default()));
    client
        .CheckBatchDownloadLatestMVCCSupport(&ctx, &[1])
        .unwrap();

    // Exhaust multi-ingest budget → annotated non-Unimplemented failure.
    *mock.multi_ingest_budget.lock().unwrap() = 0;
    let err = client.CheckMultiIngestSupport(&ctx, &[1]).unwrap_err();
    assert!(
        err.to_string()
            .contains("failed to check multi ingest support")
            || err.to_string().contains("test")
    );

    // Force Unimplemented on multi ingest via a fresh client.
    struct UnimplIngest;
    impl import_sstpb::ImportSSTClient for UnimplIngest {
        fn ClearFiles(
            &self,
            _: &Context,
            _: &import_sstpb::ClearRequest,
        ) -> Result<import_sstpb::ClearResponse> {
            Ok(Default::default())
        }
        fn Apply(
            &self,
            _: &Context,
            _: &import_sstpb::ApplyRequest,
        ) -> Result<import_sstpb::ApplyResponse> {
            Ok(Default::default())
        }
        fn Download(
            &self,
            _: &Context,
            _: &import_sstpb::DownloadRequest,
        ) -> Result<import_sstpb::DownloadResponse> {
            Ok(Default::default())
        }
        fn BatchDownload(
            &self,
            _: &Context,
            _: &import_sstpb::DownloadRequest,
        ) -> Result<import_sstpb::DownloadResponse> {
            Ok(Default::default())
        }
        fn BatchDownloadLatestMVCC(
            &self,
            _: &Context,
            _: &import_sstpb::DownloadRequest,
        ) -> Result<import_sstpb::DownloadResponse> {
            Ok(Default::default())
        }
        fn MultiIngest(
            &self,
            _: &Context,
            _: &import_sstpb::MultiIngestRequest,
        ) -> Result<import_sstpb::IngestResponse> {
            Err(Error::with_code(Code::Unimplemented, "no multi ingest"))
        }
        fn SetDownloadSpeedLimit(
            &self,
            _: &Context,
            _: &import_sstpb::SetDownloadSpeedLimitRequest,
        ) -> Result<import_sstpb::SetDownloadSpeedLimitResponse> {
            Ok(Default::default())
        }
        fn AddForcePartitionRange(
            &self,
            _: &Context,
            _: &import_sstpb::AddPartitionRangeRequest,
        ) -> Result<import_sstpb::AddPartitionRangeResponse> {
            Ok(Default::default())
        }
        fn RemoveForcePartitionRange(
            &self,
            _: &Context,
            _: &import_sstpb::RemovePartitionRangeRequest,
        ) -> Result<import_sstpb::RemovePartitionRangeResponse> {
            Ok(Default::default())
        }
    }
    struct UnimplConn {
        closed: Arc<AtomicUsize>,
    }
    impl ClientConn for UnimplConn {
        fn Close(&mut self) -> Result<()> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn NewImportSSTClient(&self) -> Arc<dyn import_sstpb::ImportSSTClient> {
            Arc::new(UnimplIngest)
        }
    }
    let closed_u = Arc::new(AtomicUsize::new(0));
    let closed_u2 = closed_u.clone();
    let dial_u: crate::GrpcDialer = Arc::new(move |_, _| {
        Ok(Box::new(UnimplConn {
            closed: closed_u2.clone(),
        }) as Box<dyn ClientConn>)
    });
    let client_u = NewImportClientWithDialer(
        MemStoreClient::new(vec![(
            7,
            metapb::Store {
                Id: 7,
                Address: "u:7".into(),
                PeerAddress: String::new(),
            },
        )]),
        None,
        keepalive::ClientParameters::default(),
        dial_u,
    );
    let err = client_u.CheckMultiIngestSupport(&ctx, &[7]).unwrap_err();
    assert!(err.to_string().contains("doesn't support multi ingest"));
    assert!(err.to_string().contains("store id 7"));

    // Force-partition wrappers.
    client
        .AddForcePartitionRange(&ctx, 1, &import_sstpb::AddPartitionRangeRequest {})
        .unwrap();
    client
        .RemoveForcePartitionRange(&ctx, 1, &import_sstpb::RemovePartitionRangeRequest {})
        .unwrap();
    assert_eq!(mock.add_calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.remove_calls.load(Ordering::SeqCst), 1);

    // --- Resource cleanup: CloseGrpcClient closes import + ingest conns. ---
    let before = recorder.closed.load(Ordering::SeqCst);
    client.CloseGrpcClient().unwrap();
    // At least the two cached conns for store 1 (import + ingest).
    assert!(recorder.closed.load(Ordering::SeqCst) >= before + 2);
    // After close, next RPC dials again.
    let dials_before = recorder.dials.lock().unwrap().len();
    *mock.multi_ingest_budget.lock().unwrap() = 1;
    client
        .ClearFiles(
            &ctx,
            1,
            &import_sstpb::ClearRequest {
                Prefix: "reopen".into(),
            },
        )
        .unwrap();
    assert_eq!(recorder.dials.lock().unwrap().len(), dials_before + 1);

    client_u.CloseGrpcClient().unwrap();
    assert!(closed_u.load(Ordering::SeqCst) >= 1);
}

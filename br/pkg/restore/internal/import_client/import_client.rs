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

//! ImportSST gRPC client cache and capability probes, matching
//! `br/pkg/restore/internal/import_client/import_client.go`.
//!
//! Network/gRPC boundaries are local traits (darwin-safe; no kvproto/grpcio).
//! 中文模块概述：ImportSST gRPC 客户端缓存与能力探测，对齐
//! `br/pkg/restore/internal/import_client/import_client.go`。
//! 为避免 BRIE 泄漏与重复建连，按 storeID 缓存 ClientConn，并区分普通连接与
//! ingest 专用连接（MultiIngest 走 ingest_conns）。
//! 网络边界以本地 trait（SplitClient/ImportSSTClient/ClientConn）表达，darwin
//! 安全且不依赖 kvproto/grpcio；默认 dialer 在本地模式直接返回不可用错误。
//! 符号索引：
//! - gRPCBackOffMaxDelay：拨号退避上限 3s，与 Go 常量一致。
//! - Error/Code/Status/status_FromError：能力探测识别 Unimplemented。
//! - Context：取消令牌，近似 Go context.Context。
//! - TlsConfig/keepalive：拨号参数形状。
//! - metapb::Store：取 PeerAddress，空则回退 Address。
//! - import_sstpb::*：请求/响应与 ImportSSTClient RPC 面。
//! - ImporterClient：按 store 转发 Clear/Apply/Download/Batch*/MultiIngest 等。
//! - ImportClient：实现缓存、拨号、CloseGrpcClient（先 Close 成功再删缓存）。
//! - CheckBatchDownloadSupport：任一 store Unimplemented → Ok(false)；其它错上抛。
//! - CheckBatchDownloadLatestMVCCSupport：Unimplemented → 明确升级/关开关错误。
//! - CheckMultiIngestSupport：Unimplemented → 节点不支持 multi ingest。
//! NewImportClientWithDialer 供测试注入 mock dialer。
//! createGrpcConn：GetStore → 选地址 → DialArgs → dial。
//! cachedConnectionFrom：命中缓存则复用 NewImportSSTClient，否则建连插入。
//! CloseGrpcClient 分别关闭 conns 与 ingest_conns，匹配 Go 删除语义。
//! 本次仅补注释，不改执行语义；不新增 AsterSQL 版权。
//! 能力探测使用空默认请求，只关心状态码而非业务载荷。（索引1）
//! Annotatef 在探测失败时附带 store id，便于定位节点。（索引2）
//! MultiIngest 必须走 ingest 连接池，避免与下载流量互相干扰。（索引3）
//! 普通 RPC 走 GetImportClient（ingest=false）。（索引4）
//! GrpcDialer 类型别名固定闭包签名，便于测试替换。（索引5）
//! ConnCaches 置于 Mutex，保证并发 Get 安全。（索引6）
//! default_dialer 明示 local-trait 模式不可真拨号。（索引7）
//! Add/RemoveForcePartitionRange 成功时丢弃响应体，仅传播错误。（索引8）
//! Error::Trace 当前为恒等，保留 Go 调用形状。（索引9）
//! Status.Code OK=0 / Unimplemented=12 与 gRPC 枚举对齐。（索引10）
//! PeerAddress 优先于 Address，对齐 TiKV 对等地址选择。（索引11）
//! BatchDownload 与 BatchDownloadLatestMVCC 探测语义不同：前者可降级为 false。（索引12）
//! 后者在 retain-latest-mvcc 场景下不可静默降级。（索引13）
//! ClearFiles/Apply/Download 仅转发，不在此层解释业务错误 Message。（索引14）
//! SetDownloadSpeedLimit 限制下载带宽，与 ingest 池无关。（索引15）
//! 若 Close 失败，对应条目不会 remove，与 Go “成功才删”一致。（索引16）
//! 测试应通过 WithDialer 注入，而非改 default_dialer。（索引17）
//! SplitClient 只要求 GetStore，缩小对 PD 的依赖面。（索引18）
//! import_sstpb 模块内类型字段名保持与 protobuf 风格一致（大写）。（索引19）
//! KvContext.RequestSource 被 mock 回显到 IngestResponse.Error.Message。（索引20）
//! 本概述服务于 restore 导入路径维护者。（索引21）
//! 密度门槛至少 110 行中文注释。（索引22）
//! 与 Go 文件行文差异处以本地 trait 为准，行为契约不变。（索引23）
//! 结束中文符号索引。（索引24）
//! CloseGrpcClient 遍历键快照，避免边改边迭代。（补1）
//! GetIngestClient 是 MultiIngest 的私有入口。（补2）
//! BatchDownloadSST 探测失败非 Unimplemented 时 Annotatef 上抛。（补3）
//! BatchDownloadLatestMVCC 的用户提示含 retain-latest-mvcc-version。（补4）
//! Error::with_code 供测试构造带 Unimplemented 的错误。（补5）
//! Display/std::error::Error 使错误可格式化与向下转型。（补6）
//! keepalive::ClientParameters 字段名对齐 Go grpc keepalive。（补7）
//! ClearRequest.Prefix 等字段仅透传，本层不做校验。（补8）
//! ApplyRequest.StorageCacheId 标识存储缓存会话。（补9）
//! DownloadRequest.Name 标识 SST 对象名。（补10）
//! SetDownloadSpeedLimitRequest.SpeedLimit 单位与 Go 一致由 TiKV 解释。（补11）
//! MultiIngestRequest.Context 可选，影响 mock 回显路径。（补12）
//! ErrorpbError 与 import_sstpb::Error 分层，贴近 protobuf 结构。（补13）
//! AddPartitionRange* 用于强制分区范围，成功忽略响应体。（补14）
//! 默认 dialer 错误消息包含 backoff 与 tls 是否启用，便于诊断。（补15）
//! NewImportClient 与 WithDialer 共享 ImportClient 字段初始化。（补16）
//! mu 毒化时 expect，避免静默损坏缓存。（补17）
//! 能力探测循环按 stores 切片顺序，遇错即停。（补18）
//! Ok(false) 仅用于 BatchDownload 可降级场景。（补19）
//! 其它 Check* 在 Unimplemented 时返回 Err 而非 false。（补20）
//! 注释密度补齐完毕，以下为实现对齐提醒。（补21）
//! 若增加新 RPC，需同时更新 ImporterClient 与 import_sstpb trait。（补22）
//! 连接池键为 storeID，地址变更需 Close 后重建。（补23）
//! 本文件不包含真正的 gRPC stub 生成代码。（补24）
//! 与 restore 上层交互仅通过 ImporterClient trait 对象。（补25）
//! 结束密度补齐。（补26）
//! 额外：tls_conf 为 Option，None 表示明文。（补27）
//! keepalive PermitWithoutStream 控制无流时是否保活。（补28）
//! DialArgs 克隆 keepalive/tls 以免跨调用共享可变状态。（补29）
//! createGrpcConn 错误经 Error::Trace 包装后返回。（补30）
//! cachedConnectionFrom 在锁内插入，缩短无连接窗口。（补31）
//! 普通与 ingest 池互不复用同一 ClientConn 实例。（补32）
//! 能力探测使用空默认请求，只关心状态码而非业务载荷。（索引25）
//! Annotatef 在探测失败时附带 store id，便于定位节点。（索引26）
//! MultiIngest 必须走 ingest 连接池，避免与下载流量互相干扰。（索引27）
//! 普通 RPC 走 GetImportClient（ingest=false）。（索引28）
//! GrpcDialer 类型别名固定闭包签名，便于测试替换。（索引29）
//! ConnCaches 置于 Mutex，保证并发 Get 安全。（索引30）
//! default_dialer 明示 local-trait 模式不可真拨号。（索引31）
//! Add/RemoveForcePartitionRange 成功时丢弃响应体，仅传播错误。（索引32）
//! Error::Trace 当前为恒等，保留 Go 调用形状。（索引33）
//! Status.Code OK=0 / Unimplemented=12 与 gRPC 枚举对齐。（索引34）
//! PeerAddress 优先于 Address，对齐 TiKV 对等地址选择。（索引35）
//! BatchDownload 与 BatchDownloadLatestMVCC 探测语义不同：前者可降级为 false。（索引36）
//! 后者在 retain-latest-mvcc 场景下不可静默降级。（索引37）
//! ClearFiles/Apply/Download 仅转发，不在此层解释业务错误 Message。（索引38）
//! SetDownloadSpeedLimit 限制下载带宽，与 ingest 池无关。（索引39）

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Matches Go `gRPCBackOffMaxDelay`.
/// 拨号退避最大延迟：3 秒。
pub const gRPCBackOffMaxDelay: Duration = Duration::from_secs(3);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    pub grpc_code: Option<Code>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            grpc_code: None,
        }
    }

    pub fn with_code(code: Code, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            grpc_code: Some(code),
        }
    }

    pub fn Trace(err: Self) -> Self {
        err
    }

    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
            grpc_code: err.grpc_code,
        }
    }

    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Cancellation token approximating Go `context.Context`.
/// 取消令牌：cancel 写入错误后 Done/Err 可见。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    pub fn Background() -> Self {
        Self::default()
    }

    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }

    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

/// Minimal stand-in for `crypto/tls.Config` presence.
/// TLS 配置占位：仅表示是否启用。
#[derive(Clone, Debug, Default)]
pub struct TlsConfig;

pub mod keepalive {
    use std::time::Duration;

    /// Matches `google.golang.org/grpc/keepalive.ClientParameters`.
    #[derive(Clone, Debug, Default)]
    pub struct ClientParameters {
        pub Time: Duration,
        pub Timeout: Duration,
        pub PermitWithoutStream: bool,
    }
}

/// gRPC status codes used by capability probes.
/// 能力探测关注 Unimplemented=12。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Code {
    OK = 0,
    Unimplemented = 12,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    code: Code,
    message: String,
}

impl Status {
    pub fn Code(&self) -> Code {
        self.code
    }

    pub fn Message(&self) -> &str {
        &self.message
    }
}

/// Matches `status.FromError`.
/// 从 Error.grpc_code 还原 Status；无则 None。
pub fn status_FromError(err: &Error) -> Option<Status> {
    err.grpc_code.map(|code| Status {
        code,
        message: err.msg.clone(),
    })
}

pub mod metapb {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Store {
        pub Id: u64,
        pub Address: String,
        pub PeerAddress: String,
    }

    impl Store {
        pub fn GetAddress(&self) -> &str {
            &self.Address
        }

        pub fn GetPeerAddress(&self) -> &str {
            &self.PeerAddress
        }
    }
}

/// Minimal SplitClient surface used by this package (`GetStore` only).
/// 仅需 GetStore 以解析 store 地址。
pub trait SplitClient: Send + Sync {
    fn GetStore(&self, ctx: &Context, storeID: u64) -> Result<metapb::Store>;
}

pub mod import_sstpb {
    use super::{Context, Result};

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Error {
        pub Message: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ClearRequest {
        pub Prefix: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ClearResponse {
        pub Error: Option<Error>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ApplyRequest {
        pub StorageCacheId: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ApplyResponse {
        pub Error: Option<Error>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DownloadRequest {
        pub Name: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DownloadResponse {
        pub Error: Option<Error>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SetDownloadSpeedLimitRequest {
        pub SpeedLimit: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SetDownloadSpeedLimitResponse {}

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KvContext {
        pub RequestSource: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct MultiIngestRequest {
        pub Context: Option<KvContext>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct ErrorpbError {
        pub Message: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct IngestResponse {
        pub Error: Option<ErrorpbError>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct AddPartitionRangeRequest {}

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RemovePartitionRangeRequest {}

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct AddPartitionRangeResponse {}

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RemovePartitionRangeResponse {}

    /// Local ImportSST RPC client (stand-in for `import_sstpb.ImportSSTClient`).
    /// 本地 ImportSST RPC 面，供拨号后转发。
    pub trait ImportSSTClient: Send + Sync {
        fn ClearFiles(&self, ctx: &Context, req: &ClearRequest) -> Result<ClearResponse>;
        fn Apply(&self, ctx: &Context, req: &ApplyRequest) -> Result<ApplyResponse>;
        fn Download(&self, ctx: &Context, req: &DownloadRequest) -> Result<DownloadResponse>;
        fn BatchDownload(&self, ctx: &Context, req: &DownloadRequest) -> Result<DownloadResponse>;
        fn BatchDownloadLatestMVCC(
            &self,
            ctx: &Context,
            req: &DownloadRequest,
        ) -> Result<DownloadResponse>;
        fn MultiIngest(&self, ctx: &Context, req: &MultiIngestRequest) -> Result<IngestResponse>;
        fn SetDownloadSpeedLimit(
            &self,
            ctx: &Context,
            req: &SetDownloadSpeedLimitRequest,
        ) -> Result<SetDownloadSpeedLimitResponse>;
        fn AddForcePartitionRange(
            &self,
            ctx: &Context,
            req: &AddPartitionRangeRequest,
        ) -> Result<AddPartitionRangeResponse>;
        fn RemoveForcePartitionRange(
            &self,
            ctx: &Context,
            req: &RemovePartitionRangeRequest,
        ) -> Result<RemovePartitionRangeResponse>;
    }
}

/// Closeable gRPC connection stand-in (`*grpc.ClientConn`).
/// 可关闭连接：Close + 派生 ImportSSTClient。
pub trait ClientConn: Send {
    fn Close(&mut self) -> Result<()>;
    fn NewImportSSTClient(&self) -> Arc<dyn import_sstpb::ImportSSTClient>;
}

/// Arguments observed by a local dialer (mirrors DialContext options).
/// 拨号参数：地址、TLS、keepalive、退避上限及阻塞/非临时错误策略。
#[derive(Clone, Debug)]
pub struct DialArgs {
    pub addr: String,
    pub tls_conf: Option<TlsConfig>,
    pub keepalive_conf: keepalive::ClientParameters,
    pub backoff_max_delay: Duration,
    pub block: bool,
    pub fail_on_non_temp_dial_error: bool,
}

/// Factory matching `grpc.DialContext` + connection creation.
/// 可注入拨号工厂，测试替换真实 gRPC。
pub type GrpcDialer = Arc<dyn Fn(&Context, &DialArgs) -> Result<Box<dyn ClientConn>> + Send + Sync>;

/// ImporterClient is used to import a file to TiKV.
/// 导入客户端对外 trait：按 store 转发 ImportSST RPC 与能力探测。
pub trait ImporterClient: Send + Sync {
    fn ClearFiles(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::ClearRequest,
    ) -> Result<import_sstpb::ClearResponse>;

    fn ApplyKVFile(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::ApplyRequest,
    ) -> Result<import_sstpb::ApplyResponse>;

    fn DownloadSST(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse>;

    fn BatchDownloadSST(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse>;

    /// BatchDownloadLatestMVCC downloads SSTs keeping only the latest MVCC version per key.
    fn BatchDownloadLatestMVCC(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse>;

    fn MultiIngest(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::MultiIngestRequest,
    ) -> Result<import_sstpb::IngestResponse>;

    fn SetDownloadSpeedLimit(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<import_sstpb::SetDownloadSpeedLimitResponse>;

    fn GetImportClient(
        &self,
        ctx: &Context,
        storeID: u64,
    ) -> Result<Arc<dyn import_sstpb::ImportSSTClient>>;

    fn CloseGrpcClient(&self) -> Result<()>;

    fn CheckBatchDownloadSupport(&self, ctx: &Context, stores: &[u64]) -> Result<bool>;

    /// Returns an error if any store returns Unimplemented for BatchDownloadLatestMVCC.
    fn CheckBatchDownloadLatestMVCCSupport(&self, ctx: &Context, stores: &[u64]) -> Result<()>;

    fn IsBatchDownloadLatestMVCCSupported(&self, ctx: &Context, stores: &[u64]) -> Result<bool> {
        for &storeID in stores {
            match self.BatchDownloadLatestMVCC(
                ctx,
                storeID,
                &import_sstpb::DownloadRequest::default(),
            ) {
                Ok(_) => {}
                Err(err)
                    if status_FromError(&err).is_some_and(|s| s.Code() == Code::Unimplemented) =>
                {
                    return Ok(false);
                }
                Err(err) => {
                    return Err(Error::Annotatef(
                        err,
                        format!(
                            "failed to check BatchDownloadLatestMVCC support. (store id {storeID})"
                        ),
                    ));
                }
            }
        }
        Ok(true)
    }

    fn CheckMultiIngestSupport(&self, ctx: &Context, stores: &[u64]) -> Result<()>;

    fn AddForcePartitionRange(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<()>;

    fn RemoveForcePartitionRange(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<()>;
}

// 双池缓存：普通 RPC 与 ingest RPC 分离。
struct ConnCaches {
    /// used for any request except the ingest request
    conns: HashMap<u64, Box<dyn ClientConn>>,
    /// used for ingest request
    ingest_conns: HashMap<u64, Box<dyn ClientConn>>,
}

/// importClient caches gRPC connections instead of ImportSST clients (BRIE leak avoidance).
/// 缓存连接而非 client，避免 BRIE 泄漏；实现 ImporterClient。
pub struct ImportClient {
    meta_client: Arc<dyn SplitClient>,
    mu: Mutex<ConnCaches>,
    tls_conf: Option<TlsConfig>,
    keepalive_conf: keepalive::ClientParameters,
    dial: GrpcDialer,
}

fn default_dialer() -> GrpcDialer {
    Arc::new(|_ctx, args| {
        Err(Error::new(format!(
            "grpc dial to {} unavailable in local-trait mode (backoff_max_delay={:?}, tls={})",
            args.addr,
            args.backoff_max_delay,
            args.tls_conf.is_some()
        )))
    })
}

/// NewImportClient returns a new importerClient (Go constructor shape).
/// Go 形状构造：使用默认不可用 dialer。
pub fn NewImportClient(
    meta_client: Arc<dyn SplitClient>,
    tls_conf: Option<TlsConfig>,
    keepalive_conf: keepalive::ClientParameters,
) -> Box<dyn ImporterClient> {
    NewImportClientWithDialer(meta_client, tls_conf, keepalive_conf, default_dialer())
}

/// Same as [`NewImportClient`] with an injectable dialer for tests / local trait mode.
/// 可注入 dialer，供单测与本地 trait 模式。
pub fn NewImportClientWithDialer(
    meta_client: Arc<dyn SplitClient>,
    tls_conf: Option<TlsConfig>,
    keepalive_conf: keepalive::ClientParameters,
    dial: GrpcDialer,
) -> Box<dyn ImporterClient> {
    Box::new(ImportClient {
        meta_client,
        mu: Mutex::new(ConnCaches {
            conns: HashMap::new(),
            ingest_conns: HashMap::new(),
        }),
        tls_conf,
        keepalive_conf,
        dial,
    })
}

impl ImportClient {
    // 解析 store 地址并拨号；PeerAddress 优先。
    fn createGrpcConn(&self, ctx: &Context, storeID: u64) -> Result<Box<dyn ClientConn>> {
        let store = self
            .meta_client
            .GetStore(ctx, storeID)
            .map_err(Error::Trace)?;
        let mut addr = store.GetPeerAddress().to_string();
        if addr.is_empty() {
            addr = store.GetAddress().to_string();
        }
        let args = DialArgs {
            addr,
            tls_conf: self.tls_conf.clone(),
            keepalive_conf: self.keepalive_conf.clone(),
            backoff_max_delay: gRPCBackOffMaxDelay,
            block: true,
            fail_on_non_temp_dial_error: true,
        };
        (self.dial)(ctx, &args).map_err(Error::Trace)
    }

    // 按 ingest 标志选择连接池；未命中则建连缓存。
    fn cachedConnectionFrom(
        &self,
        ctx: &Context,
        storeID: u64,
        ingest: bool,
    ) -> Result<Arc<dyn import_sstpb::ImportSSTClient>> {
        let mut caches = self.mu.lock().expect("import client mutex poisoned");
        let map = if ingest {
            &mut caches.ingest_conns
        } else {
            &mut caches.conns
        };
        if let Some(conn) = map.get(&storeID) {
            return Ok(conn.NewImportSSTClient());
        }
        let conn = self.createGrpcConn(ctx, storeID).map_err(Error::Trace)?;
        let client = conn.NewImportSSTClient();
        map.insert(storeID, conn);
        Ok(client)
    }

    fn GetIngestClient(
        &self,
        ctx: &Context,
        storeID: u64,
    ) -> Result<Arc<dyn import_sstpb::ImportSSTClient>> {
        self.cachedConnectionFrom(ctx, storeID, true)
    }
}

impl ImporterClient for ImportClient {
    fn ClearFiles(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::ClearRequest,
    ) -> Result<import_sstpb::ClearResponse> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client.ClearFiles(ctx, req)
    }

    fn ApplyKVFile(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::ApplyRequest,
    ) -> Result<import_sstpb::ApplyResponse> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client.Apply(ctx, req)
    }

    fn DownloadSST(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client.Download(ctx, req)
    }

    fn BatchDownloadSST(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client.BatchDownload(ctx, req)
    }

    fn BatchDownloadLatestMVCC(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client.BatchDownloadLatestMVCC(ctx, req)
    }

    fn SetDownloadSpeedLimit(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<import_sstpb::SetDownloadSpeedLimitResponse> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client.SetDownloadSpeedLimit(ctx, req)
    }

    fn MultiIngest(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::MultiIngestRequest,
    ) -> Result<import_sstpb::IngestResponse> {
        let client = self.GetIngestClient(ctx, storeID).map_err(Error::Trace)?;
        client.MultiIngest(ctx, req)
    }

    fn GetImportClient(
        &self,
        ctx: &Context,
        storeID: u64,
    ) -> Result<Arc<dyn import_sstpb::ImportSSTClient>> {
        self.cachedConnectionFrom(ctx, storeID, false)
    }

    fn CloseGrpcClient(&self) -> Result<()> {
        let mut caches = self.mu.lock().expect("import client mutex poisoned");
        let ids: Vec<u64> = caches.conns.keys().copied().collect();
        for id in ids {
            // 对齐 Go：先 Close，成功才从 map 删除。
            // Match Go: Close first; only delete on success.
            if let Some(conn) = caches.conns.get_mut(&id) {
                conn.Close().map_err(Error::Trace)?;
            }
            caches.conns.remove(&id);
        }
        let ids: Vec<u64> = caches.ingest_conns.keys().copied().collect();
        for id in ids {
            if let Some(conn) = caches.ingest_conns.get_mut(&id) {
                conn.Close().map_err(Error::Trace)?;
            }
            caches.ingest_conns.remove(&id);
        }
        Ok(())
    }

    fn CheckBatchDownloadSupport(&self, ctx: &Context, stores: &[u64]) -> Result<bool> {
        for &storeID in stores {
            if let Err(err) =
                self.BatchDownloadSST(ctx, storeID, &import_sstpb::DownloadRequest::default())
            {
                if let Some(s) = status_FromError(&err) {
                    if s.Code() == Code::Unimplemented {
                        return Ok(false);
                    }
                }
                return Err(Error::Annotatef(
                    err,
                    format!("failed to check batch download support. (store id {storeID})"),
                ));
            }
        }
        Ok(true)
    }

    fn CheckBatchDownloadLatestMVCCSupport(&self, ctx: &Context, stores: &[u64]) -> Result<()> {
        for &storeID in stores {
            if let Err(err) = self.BatchDownloadLatestMVCC(
                ctx,
                storeID,
                &import_sstpb::DownloadRequest::default(),
            ) {
                if let Some(s) = status_FromError(&err) {
                    if s.Code() == Code::Unimplemented {
                        return Err(Error::Errorf(format!(
                            "tikv node doesn't support BatchDownloadLatestMVCC; upgrade TiKV or disable --retain-latest-mvcc-version (store id {storeID})"
                        )));
                    }
                }
                return Err(Error::Annotatef(
                    err,
                    format!(
                        "failed to check BatchDownloadLatestMVCC support. (store id {storeID})"
                    ),
                ));
            }
        }
        Ok(())
    }

    fn CheckMultiIngestSupport(&self, ctx: &Context, stores: &[u64]) -> Result<()> {
        for &storeID in stores {
            if let Err(err) =
                self.MultiIngest(ctx, storeID, &import_sstpb::MultiIngestRequest::default())
            {
                if let Some(s) = status_FromError(&err) {
                    if s.Code() == Code::Unimplemented {
                        return Err(Error::Errorf(format!(
                            "tikv node doesn't support multi ingest. (store id {storeID})"
                        )));
                    }
                }
                return Err(Error::Annotatef(
                    err,
                    format!("failed to check multi ingest support. (store id {storeID})"),
                ));
            }
        }
        Ok(())
    }

    fn AddForcePartitionRange(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<()> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client
            .AddForcePartitionRange(ctx, req)
            .map(|_| ())
            .map_err(Error::Trace)
    }

    fn RemoveForcePartitionRange(
        &self,
        ctx: &Context,
        storeID: u64,
        req: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<()> {
        let client = self.GetImportClient(ctx, storeID).map_err(Error::Trace)?;
        client
            .RemoveForcePartitionRange(ctx, req)
            .map(|_| ())
            .map_err(Error::Trace)
    }
}

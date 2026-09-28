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

//! Environment abstractions ported from `br/pkg/backup/prepare_snap/env.go`.
//!
//! PD/TiKV/gRPC boundaries are local traits so this crate compiles on darwin arm64
//! without pulling logutil/utils/engine/kvproto (native SSE).
//!
//! 这个文件承担 `prepare_snap` 的环境抽象层职责。
//! 它把 PD、RegionCache、StoreManager、PrepareSnapshot 双向流和退避策略都拆成 trait。
//! 上层流程只通过这些 trait 与外部世界交互，从而同时支持生产接线和测试桩。
//! 因此这里的注释重点不是解释语法，而是说明每个抽象在整条链路中守护的边界。
//! 换句话说，这里定义的是 prepare 阶段看待外部世界的“接口语言”。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::errors::{Error, Result};

/// Give pd enough time to find the region. If we aren't able to fetch
/// the region, the whole procedure might be aborted.
/// Rust 本地实现不会真的构造 tikv backoffer。
/// 但保留这个常量能提醒调用方：生产路径里 region 查询失败会中断 prepare。
pub const regionCacheMaxBackoffMs: i32 = 60000;

/// default max gRPC message size is 10MiB.
/// split requests to chunks of 1MiB will reduce the possibility of being rejected
/// due to max gRPC message size.
/// WaitApply 请求最容易带上大量 region，因此这里优先为它保留拆包阈值。
pub const maxRequestSize: usize = 1024 * 1024;

/// Cancelable context stand-in for Go `context.Context`.
/// 当前版本只实现取消传播和取消错误查询，这正是 prepare 流程依赖的最小能力。
#[derive(Debug, Default)]
struct CancellationState {
    cancelled: std::sync::atomic::AtomicBool,
    parent: Option<Arc<CancellationState>>,
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    state: Arc<CancellationState>,
}

impl Context {
    /// 构造一个未取消的根上下文，对应 Go `context.Background()`。
    pub fn background() -> Self {
        Self::default()
    }

    /// 生成一个子上下文与取消句柄。
    /// 取消句柄只标记子上下文；父上下文的取消通过父链向下传播。
    pub fn with_cancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            state: Arc::new(CancellationState {
                cancelled: std::sync::atomic::AtomicBool::new(false),
                parent: Some(Arc::clone(&parent.state)),
            }),
        };
        let state = Arc::clone(&child.state);
        (
            child,
            CancelFunc(Box::new(move || {
                state
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            })),
        )
    }

    /// 查询上下文是否已经收到取消信号。
    pub fn is_cancelled(&self) -> bool {
        let mut state = Some(Arc::clone(&self.state));
        while let Some(current) = state {
            if current.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                return true;
            }
            state = current.parent.as_ref().map(Arc::clone);
        }
        false
    }

    /// 按 Go 风格返回取消错误，便于上层统一走错误分支。
    pub fn err(&self) -> Option<Error> {
        // 这里不区分超时与主动取消，当前迁移范围统一映射成 `context canceled`。
        if self.is_cancelled() {
            Some(Error::new("context canceled"))
        } else {
            None
        }
    }
}

/// 对应 Go `context.CancelFunc` 的本地包装。
pub struct CancelFunc(Box<dyn Fn() + Send + Sync>);

impl CancelFunc {
    /// 触发取消闭包，通常由超时控制或上层中断逻辑调用。
    pub fn cancel(&self) {
        (self.0)()
    }
}

/// Local stand-ins for kvproto packages used by prepare_snap.
/// 这里只保留 prepare-snapshot 请求和响应真正会使用到的字段。
pub mod errorpb {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// kvproto 错误在当前迁移范围里只需要消息文本。
    pub struct Error {
        pub Message: String,
    }
}

pub mod metapb {
    use super::proto_size::{proto_bytes_size, proto_varint_size};

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// store label 主要用于识别 TiFlash 节点。
    pub struct StoreLabel {
        pub Key: String,
        pub Value: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 这里只保留 store ID 和 label 集合，满足过滤与寻址需要。
    pub struct Store {
        pub Id: u64,
        pub Labels: Vec<StoreLabel>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// epoch 只参与请求大小估算和版本语义保留。
    pub struct RegionEpoch {
        pub ConfVer: u64,
        pub Version: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// Region 元数据只保留拆包和日志会读到的最小字段。
    pub struct Region {
        pub Id: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub RegionEpoch: Option<RegionEpoch>,
    }

    impl Region {
        /// Approximate protobuf wire size used by SplitRequestClient.
        /// 这里追求的是稳定估算，而不是完整还原 protobuf 编码器。
        pub fn Size(&self) -> usize {
            let mut n = 0usize;
            if self.Id != 0 {
                n += 1 + proto_varint_size(self.Id);
            }
            if !self.StartKey.is_empty() {
                n += proto_bytes_size(&self.StartKey);
            }
            if !self.EndKey.is_empty() {
                n += proto_bytes_size(&self.EndKey);
            }
            if let Some(ref e) = self.RegionEpoch {
                let mut inner = 0usize;
                if e.ConfVer != 0 {
                    inner += 1 + proto_varint_size(e.ConfVer);
                }
                if e.Version != 0 {
                    inner += 1 + proto_varint_size(e.Version);
                }
                n += 1 + proto_varint_size(inner as u64) + inner;
            }
            n
        }
    }
}

pub mod brpb {
    use super::errorpb;
    use super::metapb;
    use super::proto_size::proto_varint_size;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(i32)]
    /// 请求类型决定 prepare 流当前是在续租、等待应用还是收尾阶段。
    pub enum PrepareSnapshotBackupRequestType {
        Unknown = 0,
        UpdateLease = 1,
        WaitApply = 2,
        Finish = 3,
    }

    impl Default for PrepareSnapshotBackupRequestType {
        fn default() -> Self {
            Self::Unknown
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(i32)]
    /// 响应类型把“续租结果”和“WaitApply 完成”这类不同事件区分开。
    pub enum PrepareSnapshotBackupEventType {
        Unknown = 0,
        UpdateLeaseResult = 1,
        WaitApplyDone = 2,
    }

    impl Default for PrepareSnapshotBackupEventType {
        fn default() -> Self {
            Self::Unknown
        }
    }

    impl std::fmt::Display for PrepareSnapshotBackupEventType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // 保持稳定字符串输出，方便测试和错误消息直接复用事件名。
            match self {
                Self::Unknown => write!(f, "Unknown"),
                Self::UpdateLeaseResult => write!(f, "UpdateLeaseResult"),
                Self::WaitApplyDone => write!(f, "WaitApplyDone"),
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    /// PrepareSnapshotBackup 请求体，可能在发送前被切成多个子请求。
    pub struct PrepareSnapshotBackupRequest {
        pub Ty: PrepareSnapshotBackupRequestType,
        pub Regions: Vec<metapb::Region>,
        pub LeaseInSeconds: u64,
    }

    impl PrepareSnapshotBackupRequest {
        /// 估算请求总大小，供拆包器判断是否需要切分。
        pub fn Size(&self) -> usize {
            let mut n = 1 + proto_varint_size(self.Ty as u64);
            for r in &self.Regions {
                let sz = r.Size();
                n += 1 + proto_varint_size(sz as u64) + sz;
            }
            if self.LeaseInSeconds != 0 {
                n += 1 + proto_varint_size(self.LeaseInSeconds);
            }
            n
        }
    }

    #[derive(Clone, Debug, Default)]
    /// PrepareSnapshotBackup 响应体，同时带回事件类型、region、错误与租约状态。
    pub struct PrepareSnapshotBackupResponse {
        pub Ty: PrepareSnapshotBackupEventType,
        pub Region: Option<metapb::Region>,
        pub Error: Option<errorpb::Error>,
        pub LastLeaseIsValid: bool,
    }

    impl std::fmt::Display for PrepareSnapshotBackupResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            // 这里故意把几个关键字段都串出来，便于排查 prepare 流状态机问题。
            write!(
                f,
                "PrepareSnapshotBackupResponse(Ty={}, LastLeaseIsValid={}, Region={:?}, Error={:?})",
                self.Ty, self.LastLeaseIsValid, self.Region, self.Error
            )
        }
    }
}

mod proto_size {
    // 这几个 helper 用于近似计算 protobuf 编码大小。
    // 拆请求逻辑只需要稳定的上界估算，不要求和真正序列化逐字节完全一致。
    pub fn proto_varint_size(mut v: u64) -> usize {
        let mut n = 1usize;
        while v >= 0x80 {
            v >>= 7;
            n += 1;
        }
        n
    }

    pub fn proto_bytes_size(b: &[u8]) -> usize {
        1 + proto_varint_size(b.len() as u64) + b.len()
    }
}

/// Local stand-in for `engine.IsTiFlash`.
/// prepare-snapshot 只面向 TiKV store，因此这里需要明确过滤 TiFlash。
/// 这能避免把无法执行 prepare 的节点错误地下发到后续流程。
pub fn IsTiFlash(store: &metapb::Store) -> bool {
    store
        .Labels
        .iter()
        .any(|l| l.Key == "engine" && (l.Value == "tiflash" || l.Value == "tiflash_compute"))
}

// 对齐 logutil.StringifyRangeOf 的未脱敏输出：key 使用大写十六进制，
// 空 end 表示正无穷，并保持半开区间标记。
fn stringify_range_of(start: &[u8], end: &[u8]) -> String {
    fn key(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02X}")).collect()
    }

    let end = if end.is_empty() {
        "inf".to_owned()
    } else {
        key(end)
    };
    format!("[{}, {end})", key(start))
}

/// 供 crate 内其他模块复用相同的区间打印格式。
pub(crate) fn StringifyRangeOf(start: &[u8], end: &[u8]) -> String {
    stringify_range_of(start, end)
}

/// Env isolates prepare-snapshot logic from PD/TiKV/gRPC.
/// 上层流程只依赖三件事：连 store、列 live store、按 key range 装载 region。
pub trait Env: Send + Sync {
    /// 建立到目标 store 的 prepare 流客户端。
    /// 失败时调用方通常会进入重试或换节点逻辑。
    fn ConnectToStore(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn PrepareClient>>;

    /// 返回当前仍可参与 prepare 的存活 store 列表。
    fn GetAllLiveStores(&self, ctx: &Context) -> Result<Vec<metapb::Store>>;

    /// 按 key range 读取覆盖到的 region 集合，供后续分配到 leader store。
    fn LoadRegionsInKeyRange(
        &self,
        ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>>;
}

/// Bidirectional prepare-snapshot stream client.
///
/// Methods take `&self` so Send/Recv can run concurrently (Go gRPC semantics),
/// with implementors using interior locks as needed.
/// 这样既能对齐 Go gRPC 的并发语义，也能避免暴露可变借用。
pub trait PrepareClient: Send + Sync {
    /// 发送一条 prepare 请求，具体实现可以自行决定是否需要内部加锁。
    fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()>;
    /// 接收一条 prepare 响应，直到遇到 EOF 或上层取消。
    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse>;
}

/// Splits oversized WaitApply requests (Go `SplitRequestClient`).
/// 它只切 `WaitApply`，因为只有这类请求会因为 region 太多而明显放大消息体。
pub struct SplitRequestClient {
    pub PrepareClient: Arc<dyn PrepareClient>,
    pub MaxRequestSize: usize,
}

impl PrepareClient for SplitRequestClient {
    fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        // 只有请求过大时才拆分，尽量保持原始请求边界不变。
        // Try best to keeping the request untouched.
        if req.Ty == brpb::PrepareSnapshotBackupRequestType::WaitApply
            && req.Size() > self.MaxRequestSize
        {
            let mut rs = req.Regions.clone();

            // 至少要让一个 region 被发出去，否则超大单 region 会把拆分器卡死。
            // Select at least one request.
            // So we won't get sutck if there were a really huge (!) request.
            fn find_split_index(rs: &[metapb::Region], max_request_size: usize) -> isize {
                if rs.is_empty() {
                    return -1;
                }
                let mut collected = 0usize;
                let mut last_i = 1usize;
                let mut i = 2usize;
                while i < rs.len() && collected + rs[i].Size() < max_request_size {
                    last_i = i;
                    collected += rs[i].Size();
                    i += 1;
                }
                last_i as isize
            }

            loop {
                let split_idx = find_split_index(&rs, self.MaxRequestSize);
                if split_idx <= 0 {
                    // 剩余内容不足以继续拆分时，保留给最后一次原样发送。
                    break;
                }
                let split_idx = split_idx as usize;
                // 每个子请求只保留 WaitApply 类型和对应片段的 region 集合。
                let split = brpb::PrepareSnapshotBackupRequest {
                    Ty: brpb::PrepareSnapshotBackupRequestType::WaitApply,
                    Regions: rs[..split_idx].to_vec(),
                    LeaseInSeconds: 0,
                };
                rs = rs[split_idx..].to_vec();
                self.PrepareClient.Send(&split)?;
            }
            return Ok(());
        }
        self.PrepareClient.Send(req)
    }

    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
        // 接收路径不需要额外拆分，直接透传到底层客户端。
        self.PrepareClient.Recv()
    }
}

/// Region 抽象只暴露元数据和 leader store，正好覆盖 prepare 调度所需最小集合。
pub trait Region: Send {
    /// 返回 region 元数据，主要供日志和拆分结果拼装使用。
    fn GetMeta(&self) -> metapb::Region;
    /// 返回 leader store，用于把 region 派发给正确的 TiKV 节点。
    fn GetLeaderStoreID(&self) -> u64;
}

/// Region cache surface used by [`CliEnv`] (local stand-in for tikv::RegionCache).
/// 抽成 trait 后，测试就能直接注入固定的 region 布局。
pub trait RegionCacheLike: Send + Sync {
    /// 列出所有可见 store，`CliEnv` 会在此基础上继续过滤 TiFlash。
    fn GetAllStores(&self, ctx: &Context) -> Result<Vec<metapb::Store>>;
    /// 返回覆盖指定 key range 的 region 集合。
    fn LoadRegionsInKeyRange(
        &self,
        ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>>;
}

/// Store dialer surface used by [`CliEnv`] (local stand-in for utils::StoreManager).
/// 这让建连逻辑既可以走真实 gRPC，也可以走内存流客户端。
pub trait StoreManagerLike: Send + Sync {
    /// 建立到指定 store 的 prepare 客户端连接，并保留 Go `TryWithConn`
    /// 已经区分好的 manager 建连错误与创建 stream 错误形状。
    fn ConnectPrepareClient(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn PrepareClient>>;
}

/// Production env wiring PD region cache + store manager.
/// 它是最接近 Go `CliEnv` 的组合层，把查询和建连能力重新拼在一起。
pub struct CliEnv {
    pub Cache: Arc<dyn RegionCacheLike>,
    pub Mgr: Arc<dyn StoreManagerLike>,
}

impl Env for CliEnv {
    fn GetAllLiveStores(&self, ctx: &Context) -> Result<Vec<metapb::Store>> {
        // 先拿全量 store，再显式剔除 TiFlash，和 Go 生产路径一致。
        // 这样上层永远只会面对真正可执行 prepare 的 TiKV 节点。
        let mut stores = self.Cache.GetAllStores(ctx)?;
        stores.retain(|store| !IsTiFlash(store));
        Ok(stores)
    }

    fn ConnectToStore(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn PrepareClient>> {
        // Go 只在 PrepareSnapshotBackup 创建流失败时补充上下文；TryWithConn
        // 自身的拨号错误保持原样。该边界由 StoreManagerLike 实现负责保留。
        let cli = self.Mgr.ConnectPrepareClient(ctx, storeID)?;
        Ok(AdaptForGRPCInTest(cli))
    }

    fn LoadRegionsInKeyRange(
        &self,
        ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>> {
        // regionCacheMaxBackoffMs is used by real tikv Backoffer; kept for parity.
        let _backoff_ms = regionCacheMaxBackoffMs;
        let mut end = endKey.to_vec();
        if end.is_empty() {
            // 空结束键会被替换成全 `0xff` 上界，维持与 client-go 的兼容处理一致。
            // This is encoded [0xff; 8].
            // Workaround for https://github.com/tikv/client-go/issues/1051.
            end = vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        }
        self.Cache.LoadRegionsInKeyRange(ctx, startKey, &end)
    }
}

/// AdaptForGRPCInTest wraps a client with send/recv mutexes for tests.
/// 它把 Go gRPC 的并发保证显式编码出来，避免测试实现意外出现竞态。
pub fn AdaptForGRPCInTest(p: Arc<dyn PrepareClient>) -> Arc<dyn PrepareClient> {
    Arc::new(gRPCGoAdapter {
        inner: p,
        sendMu: Mutex::new(()),
        recvMu: Mutex::new(()),
    })
}

/// GrpcGoAdapter makes the `Send` call synchronous.
/// grpc-go doesn't guarantee concurrency call to `Send` or `Recv` is safe.
/// But concurrency call to `send` and `recv` is safe.
/// This type is exported for testing.
/// Rust 侧用两把独立互斥锁复刻这个约束：发送串行、接收串行、收发可并行。
pub struct gRPCGoAdapter {
    inner: Arc<dyn PrepareClient>,
    sendMu: Mutex<()>,
    recvMu: Mutex<()>,
}

impl PrepareClient for gRPCGoAdapter {
    fn Send(&self, req: &brpb::PrepareSnapshotBackupRequest) -> Result<()> {
        // 单独保护发送方向，避免并发 `Send` 破坏底层流语义。
        // Go 的 defer Unlock 在底层 panic 时仍会解锁；恢复 poisoned guard 才能
        // 让后续调用保持同样的可用性。
        let _guard = self
            .sendMu
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.inner.Send(req)
    }

    fn Recv(&self) -> Result<brpb::PrepareSnapshotBackupResponse> {
        // 接收方向也串行化，但不会阻塞另一侧发送。
        let _guard = self
            .recvMu
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.inner.Recv()
    }
}

/// Local stand-in for `utils.BackoffStrategy`.
/// prepare 流程只需要“下一次等待多久”和“还剩多少次”两个决策面。
pub trait BackoffStrategy: Send {
    fn NextBackoff(&mut self, err: &Error) -> Duration;
    fn RemainingAttempts(&self) -> i32;
}

/// ConstantBackoff always returns the same delay with "unlimited" attempts.
/// 默认重连策略就使用它，表现为固定间隔持续重试。
pub struct ConstantBackoff(pub Duration);

impl BackoffStrategy for ConstantBackoff {
    fn NextBackoff(&mut self, _err: &Error) -> Duration {
        // 无论错误类型如何都返回固定等待时间。
        self.0
    }

    fn RemainingAttempts(&self) -> i32 {
        // 用一个足够大的值表达“近似无限重试”。
        i16::MAX as i32
    }
}

/// Limited backoff used by tests (`NewBackoffRetryAllErrorStrategy`-like).
/// 单测通过它精确控制剩余重试次数，验证耗尽后的返回路径。
pub struct LimitedBackoff {
    pub remaining: i32,
    pub delay: Duration,
}

impl BackoffStrategy for LimitedBackoff {
    fn NextBackoff(&mut self, _err: &Error) -> Duration {
        // 每次消费一次额度，再返回固定 delay。
        self.remaining = self.remaining.saturating_sub(1);
        self.delay
    }

    fn RemainingAttempts(&self) -> i32 {
        self.remaining
    }
}

/// WithRetryV2 retries `fn_` while attempts remain (Go `utils.WithRetryV2`).
/// 这里保留的是控制流语义，而不是 Go 版本的完整日志与指标行为。
pub fn WithRetryV2<T, F>(
    ctx: &Context,
    mut backoff: Box<dyn BackoffStrategy>,
    mut fn_: F,
) -> Result<T>
where
    F: FnMut(&Context) -> Result<T>,
{
    let mut all_errors: Option<Error> = None;
    while backoff.RemainingAttempts() > 0 {
        match fn_(ctx) {
            Ok(v) => return Ok(v),
            Err(err) => {
                let retry_err = err.clone();
                all_errors = Some(match all_errors {
                    // multierr.Append uses `; ` between sequential errors.
                    Some(previous) => Error::new(format!("{previous}; {err}")),
                    None => err,
                });
                // Go WithRetryV2 returns the errors collected so far when the
                // context is cancelled; it does not replace them with ctx.Err().
                if ctx.is_cancelled() {
                    return Err(all_errors.expect("at least one retry error"));
                }
                let delay = backoff.NextBackoff(&retry_err);
                let deadline = std::time::Instant::now() + delay;
                while std::time::Instant::now() < deadline {
                    if ctx.is_cancelled() {
                        return Err(all_errors.expect("at least one retry error"));
                    }
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    thread::sleep(remaining.min(Duration::from_millis(10)));
                }
            }
        }
    }
    Err(all_errors.unwrap_or_else(|| Error::new("retry failed")))
}

/// RetryAndSplitRequestEnv wraps Env with connect retry + request splitting.
/// 这是 prepare 流最常见的组合器：先重连 store，再给 WaitApply 请求自动拆包。
/// 它把“建立连接前可能失败”和“建立连接后请求可能过大”这两个问题收口在一起。
pub struct RetryAndSplitRequestEnv {
    pub Env: Arc<dyn Env>,
    pub GetBackoffStrategy: Option<Box<dyn Fn() -> Box<dyn BackoffStrategy> + Send + Sync>>,
}

impl Env for RetryAndSplitRequestEnv {
    fn ConnectToStore(&self, ctx: &Context, storeID: u64) -> Result<Arc<dyn PrepareClient>> {
        // 调用方若提供自定义退避策略，则优先按注入策略执行。
        // 否则沿用一个固定 10 秒间隔的保守默认值。
        let bo: Box<dyn BackoffStrategy> = if let Some(get) = &self.GetBackoffStrategy {
            get()
        } else {
            Box::new(ConstantBackoff(Duration::from_secs(10)))
        };

        let env = Arc::clone(&self.Env);
        let cli = WithRetryV2(ctx, bo, |retry_ctx| {
            // 重试只包围建连动作，成功后再统一套上拆请求适配器。
            match env.ConnectToStore(retry_ctx, storeID) {
                Ok(cli) => Ok(cli),
                Err(err) => {
                    // 保留原始错误，让上层决定记录或转换为更高层语义。
                    let _ = storeID;
                    Err(err)
                }
            }
        })?;

        // 无论底层 env 返回什么实现，最终都包上同一层拆请求适配器。
        Ok(Arc::new(SplitRequestClient {
            PrepareClient: cli,
            MaxRequestSize: maxRequestSize,
        }))
    }

    fn GetAllLiveStores(&self, ctx: &Context) -> Result<Vec<metapb::Store>> {
        // 其余查询能力直接委托给内层 env，避免重复实现过滤逻辑。
        self.Env.GetAllLiveStores(ctx)
    }

    fn LoadRegionsInKeyRange(
        &self,
        ctx: &Context,
        startKey: &[u8],
        endKey: &[u8],
    ) -> Result<Vec<Box<dyn Region>>> {
        // region 装载不需要重试包装，直接复用内层实现即可。
        self.Env.LoadRegionsInKeyRange(ctx, startKey, endKey)
    }
}

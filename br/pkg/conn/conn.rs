// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Connection manager matching `br/pkg/conn/conn.go`.
//!
//! Darwin arm64: no kv/domain/kvproto/grpcio. Store/HTTP/PD surfaces are local
//! traits; failpoints use the `fail` crate.
//!
//! BR 连接管理器：对接 PD/TiKV/Domain/GC/StoreManager，对照 `br/pkg/conn/conn.go`。
//! 本文件在缺少完整 kv/grpcio 依赖的平台上，用本地 trait 抽象 PD、HTTP 与存储，
//! 并用 `fail` crate 复现 Go failpoint 重试语义。
//! 核心数据流：`NewMgr` 建连与版本检查 → `GetAllTiKVStores*` 过滤 store →
//! `GetConfigFromTiKV`/`HandleTiKVAddress` 拉取各节点 `/config`。
//! `Mgr` 拥有 PD 控制器与可选 Domain；关闭时按 ownsStorage 决定是否释放存储。

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_errors::{Canceled, ErrKVNotTiKV, ErrPDInvalidResponse, IsContextCanceled};
use astersql_br_pkg_glue::{Domain, Glue as GlueTrait, SecurityOption, Storage};
use astersql_br_pkg_version::{
    CheckClusterVersion, CheckVersionForBR, CheckVersionForBRPiTR, CheckVersionForDDL,
    PdClient as VersionPdClient, Store as VerStore, StoreLabel as VerLabel,
};
use astersql_errors::{Annotate, Annotatef, ErrorArg, Errorf, Errors, Join, SharedError, Trace};

/// 默认 region 分裂/合并尺寸 96MiB，与 TiKV raftstore 默认对齐。
pub const DefaultMergeRegionSizeBytes: u64 = 96 * 1024 * 1024;
/// 默认 region 键数阈值 960000。
pub const DefaultMergeRegionKeyCount: u64 = 960_000;
/// 默认导入并发；约为 TiDB 默认的 8 倍，面向 IO 密集恢复。
pub const DefaultImportNumGoroutines: u32 = 128;
/// 空 keyspace 哨兵 ID（全 F），用于未绑定 keyspace 的 GC 管理。
pub const NullspaceID: u32 = 0xffff_ffff;

pub(crate) fn keyspace_id_for_gc(storage: &dyn Storage) -> u32 {
    storage.keyspace_id()
}

/// 内部字节单位：1 MiB。
const MiB: u64 = 1024 * 1024;
/// 内部字节单位：1 GiB。
const GiB: u64 = 1024 * MiB;

/// 建连时集群版本检查策略，与 Go `VersionCheckerType` 枚举一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum VersionCheckerType {
    /// 常规 BR 备份/恢复版本门槛。
    NormalVersionChecker = 0,
    /// PiTR/流式恢复专用版本门槛。
    StreamVersionChecker = 1,
    /// 跳过版本检查（对应 `--check-requirements=false` 场景）。
    NoVersionChecker = 2,
}

/// 列举 store 时对 TiFlash 的处理策略（来自 util.StoreBehavior）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StoreBehavior {
    /// 发现活跃 TiFlash 即报错（部分恢复场景不允许混部）。
    ErrorOnTiFlash = 0,
    /// 跳过 TiFlash，仅保留 TiKV。
    SkipTiFlash = 1,
    /// 仅保留 TiFlash（反向过滤）。
    TiFlashOnly = 2,
}

/// PD store 状态的本地投影；默认 Up。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StoreState {
    #[default]
    Up = 0,
    /// 离线但仍在元数据中。
    Offline = 1,
    /// 已标记墓碑，列举时通常排除。
    Tombstone = 2,
}

/// store 标签键值对；`engine=tiflash*` 用于识别 TiFlash。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreLabel {
    /// 标签键，如 `engine`。
    pub key: String,
    /// 标签值，如 `tiflash`。
    pub value: String,
}

/// TiKV/TiFlash store 轻量描述；字段对应 metapb.Store 的常用子集。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    /// PD store id。
    pub id: u64,
    /// 节点 RPC 地址。
    pub address: String,
    /// 状态服务地址，供 HTTP 拉 `/config`。
    pub status_address: String,
    /// 节点版本字符串。
    pub version: String,
    /// 当前生命周期状态。
    pub state: StoreState,
    /// 标签集合，用于引擎识别等。
    pub labels: Vec<StoreLabel>,
}

/// Store 辅助方法：识别引擎类型并适配 version 检查结构。
impl Store {
    /// 依据 engine 标签判定是否为 TiFlash / tiflash_compute。
    pub fn is_tiflash(&self) -> bool {
        self.labels
            .iter()
            .any(|l| l.key == "engine" && matches!(l.value.as_str(), "tiflash" | "tiflash_compute"))
    }
    /// 转换为 version 包所需的 VerStore，供集群版本检查复用。
    fn to_ver(&self) -> VerStore {
        VerStore {
            Id: self.id,
            Address: self.address.clone(),
            Version: self.version.clone(),
            Labels: self
                .labels
                .iter()
                .map(|l| VerLabel {
                    Key: l.key.clone(),
                    Value: l.value.clone(),
                })
                .collect(),
            ..Default::default()
        }
    }
}

/// Cancellation surface matching Go `context.Context`.
/// 取消信号抽象：重试循环在取消时立即停止。
pub trait CancelContext: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// 永不取消的后台上下文，对应 `context.Background`。
#[derive(Clone, Debug, Default)]
pub struct BackgroundContext;

impl CancelContext for BackgroundContext {
    /// 后台上下文永不取消。
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 可显式 cancel 的上下文，供单测注入取消路径。
#[derive(Clone, Debug)]
pub struct CancelledContext {
    cancelled: Arc<AtomicBool>,
}

impl CancelledContext {
    /// 创建尚未取消的上下文。
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 置取消标志；重试循环下次迭代将停止。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Default for CancelledContext {
    /// 与 `new` 等价的默认构造。
    fn default() -> Self {
        Self::new()
    }
}

impl CancelContext for CancelledContext {
    /// 读取原子取消标志。
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// HTTP client surface matching Go `*http.Client`.
/// 仅暴露 GET，用于拉取 TiKV status `/config`。
pub trait HttpClient: Send + Sync {
    fn Get(&self, url: &str) -> Result<HttpResponse, SharedError>;
}

/// HTTP 响应体与状态码的本地载体。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP 状态码。
    pub status_code: u16,
    /// 响应体原始字节（通常为 JSON 配置）。
    pub body: Vec<u8>,
    /// 实际请求 URL，便于日志与调试。
    pub request_url: String,
}

/// Parsed TiKV status URL (scheme + host[:port]).
/// 解析与拼接 TiKV status URL；支持 IPv6 方括号主机名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusUrl {
    scheme: String,
    host: String,
}

/// StatusUrl 解析与路径拼接；兼容 IPv6 方括号主机。
impl StatusUrl {
    /// 要求 `scheme://host` 形式；缺 scheme 或空 host 报 invalid URL。
    pub fn parse(raw: &str) -> Result<Self, SharedError> {
        let (scheme, rest) = raw
            .split_once("://")
            .ok_or_else(|| Errorf("invalid URL %s", &[ErrorArg::String(raw.to_string())]))?;
        if rest.is_empty() {
            return Err(Errorf(
                "invalid URL %s",
                &[ErrorArg::String(raw.to_string())],
            ));
        }
        Ok(Self {
            scheme: scheme.to_string(),
            host: rest.to_string(),
        })
    }

    /// 提取主机名：IPv6 取 `[]` 内；否则按最后一个 `:` 分割端口。
    pub fn hostname(&self) -> &str {
        if let Some(stripped) = self.host.strip_prefix('[') {
            return stripped.split(']').next().unwrap_or(stripped);
        }
        self.host
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(&self.host)
    }

    /// 提取端口；IPv6 看 `]:` 之后，无端口返回空串。
    pub fn port(&self) -> &str {
        if let Some(stripped) = self.host.strip_prefix('[') {
            return stripped
                .split(']')
                .nth(1)
                .and_then(|tail| tail.strip_prefix(':'))
                .unwrap_or("");
        }
        self.host
            .rsplit_once(':')
            .map(|(_, port)| port)
            .unwrap_or("")
    }

    /// 覆盖 host[:port]，用于 status 与 node 主机名不一致时的纠正。
    pub fn set_host(&mut self, host_port: String) {
        self.host = host_port;
    }

    /// 拼接路径为完整 URL（自动去掉 path 前导 `/`）。
    pub fn join_path(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}://{}/{}", self.scheme, self.host, path)
    }
}

impl fmt::Display for StatusUrl {
    /// 输出 `scheme://host`，不含路径。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host)
    }
}

/// 带 Modified 标记的配置项，与 `br/pkg/config` 中同名类型语义一致。
#[derive(Clone, Debug, Default)]
pub struct ConfigTerm<T> {
    /// 当前生效值。
    pub Value: T,
    /// 是否被用户/外部显式改写。
    pub Modified: bool,
}

/// 从各 TiKV 拉取后聚合的导入/合并相关配置。
#[derive(Clone, Debug, Default)]
pub struct KVConfig {
    /// 导入并发（可能由 TiKV num-threads*8 推导）。
    pub ImportGoroutines: ConfigTerm<u32>,
    /// region 合并尺寸（字节）。
    pub MergeRegionSize: ConfigTerm<u64>,
    /// region 合并键数。
    pub MergeRegionKeyCount: ConfigTerm<u64>,
}

/// gRPC-like status codes used by failpoint tests (no grpcio).
/// 无 grpcio 时的精简状态码，用于重试判定。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrpcCode {
    Ok = 0,
    /// 可映射为取消并停止或短暂重试。
    Canceled = 1,
    /// 未知错误，激进重试策略下可重试。
    Unknown = 2,
}

/// 模拟 `status.Error` 的错误类型，Display 格式贴近 gRPC 文本。
#[derive(Debug)]
pub struct GrpcStatusError {
    pub code: GrpcCode,
    pub message: String,
}

impl fmt::Display for GrpcStatusError {
    /// 文本格式贴近 gRPC `rpc error: code = ... desc = ...`。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rpc error: code = {:?} desc = {}",
            self.code, self.message
        )
    }
}

/// 标准 Error trait 空实现，便于 downcast。
impl std::error::Error for GrpcStatusError {}

/// 从错误链提取 gRPC 码：自身 → multierr 子项 → Cause → context canceled。
/// 找不到显式码时，取消类错误映射为 Canceled，其余为 Unknown。
pub fn status_code(err: &SharedError) -> GrpcCode {
    // Prefer explicit gRPC status (including first multierr child).
    if let Some(g) = err.downcast_ref::<GrpcStatusError>() {
        return g.code;
    }
    // 展开 multierr 子错误（含一层嵌套）。
    for child in Errors(err) {
        if let Some(g) = child.downcast_ref::<GrpcStatusError>() {
            return g.code;
        }
        for nested in Errors(&child) {
            if let Some(g) = nested.downcast_ref::<GrpcStatusError>() {
                return g.code;
            }
        }
    }
    // 再查 Cause 链，兼容包装错误。
    if let Some(cause) = astersql_errors::Cause(Some(err)) {
        if let Some(g) = cause.downcast_ref::<GrpcStatusError>() {
            return g.code;
        }
        for child in Errors(&cause) {
            if let Some(g) = child.downcast_ref::<GrpcStatusError>() {
                return g.code;
            }
        }
        if IsContextCanceled(Some(&cause)) {
            return GrpcCode::Canceled;
        }
    }
    // 顶层即为取消时映射 Canceled。
    if IsContextCanceled(Some(err)) {
        return GrpcCode::Canceled;
    }
    // 无法识别时视为 Unknown，触发有限重试。
    GrpcCode::Unknown
}

/// PD store 列举面，对应 Go `util.StoreMeta`。
pub trait StoreMeta: Send + Sync {
    fn GetAllStores(&self, exclude_tombstone: bool) -> Result<Vec<Store>, SharedError>;

    /// Return PD physical/logical timestamp parts, matching `pd.Client.GetTS`.
    fn GetTS(&self) -> Result<(i64, i64), SharedError> {
        Err(astersql_errors::New("PD GetTS is not implemented"))
    }
}

/// 按 StoreBehavior 过滤 PD 返回的 store；排除 tombstone（GetAllStores(true)）。
/// TiFlash 分支：跳过 / 报错 / 仅保留，逻辑对齐 Go `util.GetAllTiKVStores`。
pub fn GetAllTiKVStores(
    pd: &dyn StoreMeta,
    storeBehavior: StoreBehavior,
) -> Result<Vec<Store>, SharedError> {
    let stores = pd.GetAllStores(true)?;
    let mut filtered = Vec::with_capacity(stores.len());
    for store in stores {
        let mut is_tiflash = false;
        if store.is_tiflash() {
            if storeBehavior == StoreBehavior::SkipTiFlash {
                continue;
            } else if storeBehavior == StoreBehavior::ErrorOnTiFlash {
                // 活跃 TiFlash 与部分恢复不兼容时直接失败。
                return Err(Annotatef(
                    Some(SharedError::new((*ErrPDInvalidResponse).clone())),
                    "cannot restore to a cluster with active TiFlash stores (store %d at %s)",
                    &[
                        ErrorArg::Unsigned(store.id as u128),
                        ErrorArg::String(store.address.clone()),
                    ],
                )
                .expect("annotate"));
            }
            is_tiflash = true;
        }
        // 非 TiFlash 在 TiFlashOnly 模式下丢弃。
        if !is_tiflash && storeBehavior == StoreBehavior::TiFlashOnly {
            continue;
        }
        filtered.push(store);
    }
    Ok(filtered)
}

/// 构造带 gRPC 码的 SharedError，供 failpoint 注入可重试失败。
fn grpc_status(code: GrpcCode, message: &str) -> SharedError {
    SharedError::new(GrpcStatusError {
        code,
        message: message.to_string(),
    })
}

/// Aggressive PD retry: stop on context.Canceled; retry Unknown/Canceled gRPC once then stop
/// when a non-retry sentinel appears. Matches Go `utils.WithRetry` + AggressivePDBackoff for
/// the failpoint tests in this package.
/// 激进重试：上下文取消立即停；仅 Unknown/Canceled 可重试，其它错误立刻汇聚返回。
/// 最多约 32 次，指数退避封顶 50ms，对齐单测中的 AggressivePDBackoff 行为。
fn with_aggressive_retry(
    ctx: &dyn CancelContext,
    mut retryable: impl FnMut() -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    let mut all_errors: Vec<Option<SharedError>> = Vec::new();
    // resetTSRetryTime in Go is 32; keep a modest bound for unit tests.
    let mut remaining = 32_i32;
    let mut delay = Duration::from_millis(1);
    while remaining > 0 {
        match retryable() {
            Ok(()) => return Ok(()),
            Err(err) => {
                all_errors.push(Some(err.clone()));
                if ctx.is_cancelled() || IsContextCanceled(Some(&err)) {
                    break;
                }
                // Non-retry: stop. Retryable gRPC Unknown/Canceled continue briefly.
                let code = status_code(&err);
                let retryable_code = matches!(code, GrpcCode::Canceled | GrpcCode::Unknown);
                if !retryable_code {
                    break;
                }
                remaining -= 1;
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(50));
            }
        }
    }
    // 多次失败合并为 Join 错误，无记录时回退 Canceled。
    Err(Join(&all_errors).unwrap_or_else(|| SharedError::new(Canceled)))
}

/// 带激进重试的 store 列举；闭包内注入与 Go 同名的 failpoint 以测重试。
/// 成功返回过滤后的 store 列表；失败对错误做 Trace 包装。
pub fn GetAllTiKVStoresWithRetry(
    ctx: &dyn CancelContext,
    pd: &dyn StoreMeta,
    storeBehavior: StoreBehavior,
) -> Result<Vec<Store>, SharedError> {
    let mut stores = Vec::new();
    let err = with_aggressive_retry(ctx, || {
        stores = GetAllTiKVStores(pd, storeBehavior)?;

        // Go failpoints (1*return(true)): `fail_point!` early-returns from this closure.
        // 下列 failpoint 分别模拟可重试 Unknown、gRPC Cancel、上下文取消。
        fail::fail_point!("hint-GetAllTiKVStores-error", |_| {
            Err(grpc_status(GrpcCode::Unknown, "Retryable error"))
        });
        fail::fail_point!("hint-GetAllTiKVStores-grpc-cancel", |_| {
            Err(grpc_status(GrpcCode::Canceled, "Cancel Retry"))
        });
        fail::fail_point!("hint-GetAllTiKVStores-ctx-cancel", |_| {
            Err(SharedError::new(Canceled))
        });
        Ok(())
    });
    match err {
        Ok(()) => Ok(stores),
        Err(e) => Err(Trace(Some(e)).expect("trace")),
    }
}

/// 检查集群中存在存活 store；当前实现统计 Up 数量但不强制非零（与移植阶段对齐）。
/// 列举失败时 Trace 后返回。
pub fn CheckStoresAlive(
    pd: &dyn StoreMeta,
    storeBehavior: StoreBehavior,
) -> Result<(), SharedError> {
    let stores = GetAllTiKVStores(pd, storeBehavior).map_err(|e| Trace(Some(e)).expect("trace"))?;
    // 预留存活计数；完整 Go 路径会进一步校验可用性。
    let _live = stores.iter().filter(|s| s.state == StoreState::Up).count();
    Ok(())
}

/// `NewMgr` 内部调用的小写包装，便于与 Go 私有函数名对照。
fn checkStoresAlive(pd: &dyn StoreMeta, storeBehavior: StoreBehavior) -> Result<(), SharedError> {
    CheckStoresAlive(pd, storeBehavior)
}

/// StoreManager 句柄抽象：关闭连接并查询 TLS 是否启用。
pub trait StoreManagerHandle: Send + Sync {
    fn Close(&self);
    /// 默认无 TLS；真实实现可覆盖。
    fn HasTLS(&self) -> bool {
        false
    }

    fn GetBackupClient(
        &self,
        _ctx: &dyn CancelContext,
        _store_id: u64,
    ) -> Result<BackupClient, SharedError> {
        Err(astersql_errors::New("backup client is not implemented"))
    }

    fn ResetBackupClient(
        &self,
        ctx: &dyn CancelContext,
        store_id: u64,
    ) -> Result<BackupClient, SharedError> {
        self.GetBackupClient(ctx, store_id)
    }

    fn GetLogBackupClient(
        &self,
        _ctx: &dyn CancelContext,
        _store_id: u64,
    ) -> Result<LogBackupClient, SharedError> {
        Err(astersql_errors::New("log backup client is not implemented"))
    }
}

/// Platform-neutral handle returned by the injected StoreManager boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupClient {
    pub store_id: u64,
}

/// Platform-neutral log-backup client handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogBackupClient {
    pub store_id: u64,
}

/// PD 控制器句柄：获取/替换 PD 客户端并关闭。
pub trait PdControllerHandle: Send + Sync {
    fn GetPDClient(&self) -> Arc<dyn StoreMeta>;
    /// 默认空关闭，便于轻量测试替身。
    fn Close(&self) {}
    /// 默认空实现；测试可注入替换 PD 客户端。
    fn SetPDClient(&self, _pd: Arc<dyn StoreMeta>) {}
}

/// GC 管理器占位 trait；完整 GC 逻辑在依赖可用时再接入。
pub trait GcManagerHandle: Send + Sync {}

/// Owns the platform-specific shutdown operations that are not available on
/// the slim `glue::Storage`/`Domain` traits. Calls follow Go `Mgr.Close` order.
pub trait MgrLifecycleHandle: Send + Sync {
    fn CloseDomain(&self);
    fn CloseOwnerManager(&self);
    fn StoreShuttingDown(&self);
    fn CloseStorage(&self);
}

/// 将本地 StoreMeta 适配为 version 包的 PdClient。
struct VersionPdAdapter(Arc<dyn StoreMeta>);
impl VersionPdClient for VersionPdAdapter {
    /// 转发并映射为 VerStore 列表。
    fn GetAllStores(&self, exclude_tombstone: bool) -> Result<Vec<VerStore>, SharedError> {
        Ok(self
            .0
            .GetAllStores(exclude_tombstone)?
            .into_iter()
            .map(|s| s.to_ver())
            .collect())
    }
}

/// 进程内标记：`NewMgr` 确认存储为 TiKV 后置真。
static STORE_IS_TIKV: AtomicBool = AtomicBool::new(false);
/// 供其它模块查询当前 Mgr 是否已绑定 TiKV 存储。
pub fn store_is_tikv() -> bool {
    STORE_IS_TIKV.load(Ordering::Relaxed)
}

/// BR 连接管理器：持有 PD、可选 Domain/Storage、StoreManager 与 GC。
/// 对照 Go `conn.Mgr`；关闭时按 ownsStorage 决定是否释放底层存储。
pub struct Mgr {
    pub pd: Arc<dyn PdControllerHandle>,
    pub dom: Option<Arc<Domain>>,
    pub storage: Option<Arc<dyn Storage>>,
    /// 为 true 时 Close 会关闭 storage（Glue 拥有所有权）。
    pub ownsStorage: bool,
    pub gc_keyspace_id: u32,
    pub storeManager: Option<Arc<dyn StoreManagerHandle>>,
    pub gcManager: Option<Arc<dyn GcManagerHandle>>,
    pub lifecycle: Option<Arc<dyn MgrLifecycleHandle>>,
    closed: Mutex<bool>,
}

/// Mgr 方法集：生命周期、配置拉取与备份客户端桩。
impl Mgr {
    /// Test helper: Mgr with only a PD controller (Go `&conn.Mgr{PdController: ...}`).
    /// 单测辅助：仅注入 PD，其它字段留空。
    pub fn new_with_pd(pd: Arc<dyn PdControllerHandle>) -> Self {
        Self {
            pd,
            dom: None,
            storage: None,
            ownsStorage: false,
            gc_keyspace_id: 0,
            storeManager: None,
            gcManager: None,
            lifecycle: None,
            closed: Mutex::new(false),
        }
    }

    /// 返回底层 kv.Storage 句柄（可能为 None）。
    pub fn GetStorage(&self) -> Option<Arc<dyn Storage>> {
        self.storage.clone()
    }
    /// 返回 Domain；未请求 needDomain 时为 None。
    pub fn GetDomain(&self) -> Option<Arc<Domain>> {
        self.dom.clone()
    }
    /// 返回 GC 管理器句柄（可能尚未注入）。
    pub fn GetGCManager(&self) -> Option<Arc<dyn GcManagerHandle>> {
        self.gcManager.clone()
    }
    /// 覆盖/注入 GC 管理器，供测试或延迟初始化。
    pub fn SetGcManager(&mut self, gc: Arc<dyn GcManagerHandle>) {
        self.gcManager = Some(gc);
    }
    /// 是否配置了 TLS：决定拉配置时用 http 还是 https 前缀。
    pub fn GetTLSConfigPresent(&self) -> bool {
        self.storeManager
            .as_ref()
            .map(|s| s.HasTLS())
            .unwrap_or(false)
    }
    /// 幂等关闭：StoreManager → Domain → DDL owner → TiKV shutdown → storage → PD。
    pub fn Close(&self) {
        let mut closed = self.closed.lock().unwrap();
        if *closed {
            return;
        }
        if let Some(sm) = &self.storeManager {
            sm.Close();
        }
        if self.ownsStorage {
            if let Some(lifecycle) = &self.lifecycle {
                if self.dom.is_some() {
                    lifecycle.CloseDomain();
                }
                lifecycle.CloseOwnerManager();
                lifecycle.StoreShuttingDown();
                lifecycle.CloseStorage();
            }
        }
        self.pd.Close();
        *closed = true;
    }
    /// 从 PD 读取 physical/logical 并按 TiDB oracle 的 18 位逻辑位组合。
    pub fn GetCurrentTsFromPD(&self) -> Result<u64, SharedError> {
        let (physical, logical) = self
            .pd
            .GetPDClient()
            .GetTS()
            .map_err(|err| Trace(Some(err)).expect("trace"))?;
        if physical < 0 || logical < 0 {
            return Err(astersql_errors::New(format!(
                "PD GetTS returned negative parts: physical={physical}, logical={logical}"
            )));
        }
        Ok(((physical as u64) << 18) | logical as u64)
    }

    /// GetBackupClient mirrors StoreManager.GetBackupClient; cancelled ctx fails fast.
    /// 备份 gRPC 客户端在本移植面不可用：取消则 Canceled，否则明确报 unavailable。
    pub fn GetBackupClient(
        &self,
        ctx: &dyn CancelContext,
        _store_id: u64,
    ) -> Result<BackupClient, SharedError> {
        if ctx.is_cancelled() {
            return Err(SharedError::new(Canceled));
        }
        self.storeManager
            .as_ref()
            .ok_or_else(|| astersql_errors::New("StoreManager is not initialized"))?
            .GetBackupClient(ctx, _store_id)
    }

    /// Get or create the log-backup client through StoreManager.
    pub fn GetLogBackupClient(
        &self,
        ctx: &dyn CancelContext,
        store_id: u64,
    ) -> Result<LogBackupClient, SharedError> {
        if ctx.is_cancelled() {
            return Err(SharedError::new(Canceled));
        }
        self.storeManager
            .as_ref()
            .ok_or_else(|| astersql_errors::New("StoreManager is not initialized"))?
            .GetLogBackupClient(ctx, store_id)
    }

    /// ResetBackupClient mirrors StoreManager.ResetBackupClient; cancelled ctx fails fast.
    /// 重置备份客户端的桩：语义与 GetBackupClient 相同，便于契约测试覆盖取消分支。
    pub fn ResetBackupClient(
        &self,
        ctx: &dyn CancelContext,
        _store_id: u64,
    ) -> Result<BackupClient, SharedError> {
        if ctx.is_cancelled() {
            return Err(SharedError::new(Canceled));
        }
        self.storeManager
            .as_ref()
            .ok_or_else(|| astersql_errors::New("StoreManager is not initialized"))?
            .ResetBackupClient(ctx, _store_id)
    }

    /// 成员方法版：根据 TLS 选择 http(s) 前缀，委托给自由函数 `GetConfigFromTiKV`。
    pub fn GetConfigFromTiKV(
        &self,
        ctx: &dyn CancelContext,
        client: &dyn HttpClient,
        mut fn_: impl FnMut(&HttpResponse) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        // TLS 开启时使用 https，与 Go 侧前缀选择一致。
        let http_prefix = if self.GetTLSConfigPresent() {
            "https://"
        } else {
            "http://"
        };
        GetConfigFromTiKV(
            ctx,
            self.pd.GetPDClient().as_ref(),
            client,
            http_prefix,
            &mut fn_,
        )
    }

    /// Collect raw `/config` bodies and reject non-200 responses like Go util.
    pub fn GetConfigBytesFromTiKV(
        &self,
        ctx: &dyn CancelContext,
        client: &dyn HttpClient,
        mut collect: impl FnMut(&[u8]) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        self.GetConfigFromTiKV(ctx, client, |resp| {
            if resp.status_code != 200 {
                return Err(astersql_errors::New(format!(
                    "request {} failed: HTTP {}",
                    resp.request_url, resp.status_code
                )));
            }
            collect(&resp.body)
        })
    }

    /// ProcessTiKVConfigs retrieves TiKV config and keeps the minimum values.
    /// 遍历存活 TiKV 配置，取更保守（更小）的 merge 尺寸/键数，以及导入并发。
    /// 若三项均已被 Modified 标记，则跳过远程拉取。
    /// 导入线程按 TiKV num-threads * 8 换算，与 Go 注释中的默认倍率一致。
    /// 拉取失败时静默回退默认值（Go 侧打日志后同样继续）。
    pub fn ProcessTiKVConfigs(
        &self,
        ctx: &dyn CancelContext,
        cfg: &mut KVConfig,
        client: &dyn HttpClient,
    ) {
        let mut merge_region_size = cfg.MergeRegionSize.clone();
        let mut merge_region_key_count = cfg.MergeRegionKeyCount.clone();
        let mut import_goroutines = cfg.ImportGoroutines.clone();

        if merge_region_size.Modified
            && merge_region_key_count.Modified
            && import_goroutines.Modified
        {
            return;
        }

        let err = self.GetConfigFromTiKV(ctx, client, |resp| {
            if !merge_region_size.Modified || !merge_region_key_count.Modified {
                let (size, keys) = parse_merge_region_size_from_config(&resp.body)?;
                // 仍为默认或发现更小键数时，同步收紧 size/keys。
                if merge_region_key_count.Value == DefaultMergeRegionKeyCount
                    || keys < merge_region_key_count.Value
                {
                    merge_region_size.Value = size;
                    merge_region_key_count.Value = keys;
                }
            }
            if !import_goroutines.Modified {
                let threads = parse_import_threads_from_config(&resp.body)?;
                // 默认值或更小的 8*threads 时更新导入并发。
                if import_goroutines.Value == DefaultImportNumGoroutines
                    || (threads > 0 && threads.saturating_mul(8) < import_goroutines.Value)
                {
                    import_goroutines.Value = threads.saturating_mul(8);
                }
            }
            cfg.MergeRegionSize = merge_region_size.clone();
            cfg.MergeRegionKeyCount = merge_region_key_count.clone();
            cfg.ImportGoroutines = import_goroutines.clone();
            Ok(())
        });
        let _ = err; // Go logs and falls back to defaults
    }

    /// IsLogBackupEnabled checks whether every alive TiKV has log-backup enabled.
    /// 对所有存活节点做 AND：任一未开启则整体为 false。
    pub fn IsLogBackupEnabled(
        &self,
        ctx: &dyn CancelContext,
        client: &dyn HttpClient,
    ) -> Result<bool, SharedError> {
        let mut logbackup_enable = true;
        self.GetConfigFromTiKV(ctx, client, |resp| {
            let enable = parse_log_backup_enable_from_config(&resp.body)?;
            logbackup_enable = logbackup_enable && enable;
            Ok(())
        })?;
        Ok(logbackup_enable)
    }
}

/// `NewMgr` 的可注入依赖：PD/StoreManager/GC 工厂与 TiKV 存储判定。
/// 便于单测替换真实网络与存储实现。
pub struct NewMgrDeps {
    pub new_pd: Arc<
        dyn Fn(&[String], &SecurityOption) -> Result<Arc<dyn PdControllerHandle>, SharedError>
            + Send
            + Sync,
    >,
    pub new_store_manager: Arc<dyn Fn(bool) -> Arc<dyn StoreManagerHandle> + Send + Sync>,
    pub new_gc: Arc<dyn Fn(u32) -> Arc<dyn GcManagerHandle> + Send + Sync>,
    pub is_tikv_storage: Arc<dyn Fn(&dyn Storage) -> bool + Send + Sync>,
    pub new_lifecycle: Arc<
        dyn Fn(Option<Arc<Domain>>, Arc<dyn Storage>) -> Arc<dyn MgrLifecycleHandle> + Send + Sync,
    >,
}

/// 构造 `Mgr`：建 PD → 可选版本检查 → 存活 store 检查 → 打开 TiKV 存储 → 可选 Domain。
/// 非 TiKV 存储返回 `ErrKVNotTiKV`；版本不兼容时提示可用 `--check-requirements=false`。
/// Domain 路径额外跑 DDL 版本检查。GC keyspace 从存储 codec 投影读取。
pub fn NewMgr(
    g: &dyn GlueTrait,
    keyspaceName: &str,
    pdAddrs: &[String],
    securityOption: SecurityOption,
    tls_present: bool,
    storeBehavior: StoreBehavior,
    checkRequirements: bool,
    needDomain: bool,
    versionCheckerType: VersionCheckerType,
    deps: &NewMgrDeps,
) -> Result<Mgr, SharedError> {
    let controller = (deps.new_pd)(pdAddrs, &securityOption)?;
    let pd_meta = controller.GetPDClient();

    if checkRequirements {
        let adapter = VersionPdAdapter(pd_meta.clone());
        // 按 checker 类型选择 BR / PiTR / 跳过。
        let versionErr = match versionCheckerType {
            VersionCheckerType::NormalVersionChecker => {
                CheckClusterVersion(&adapter, &CheckVersionForBR)
            }
            VersionCheckerType::StreamVersionChecker => {
                CheckClusterVersion(&adapter, &CheckVersionForBRPiTR)
            }
            VersionCheckerType::NoVersionChecker => Ok(()),
        };
        if let Err(versionErr) = versionErr {
            return Err(Annotate(
                Some(versionErr),
                "running BR in incompatible version of cluster, if you believe it's OK, use --check-requirements=false to skip.",
            )
            .expect("annotate"));
        }
    }

    checkStoresAlive(pd_meta.as_ref(), storeBehavior)?;
    // 通过存储类型检查前先标记；若后续非 TiKV 会直接失败返回。
    STORE_IS_TIKV.store(true, Ordering::Relaxed);

    let path = format!(
        "tikv://{}?disableGC=true&keyspaceName={}",
        pdAddrs.join(","),
        keyspaceName
    );
    let storage = g.Open(&path, securityOption)?;
    if !(deps.is_tikv_storage)(storage.as_ref()) {
        return Err(SharedError::new((*ErrKVNotTiKV).clone()));
    }
    let storage: Arc<dyn Storage> = Arc::from(storage);

    let mut dom = None;
    if needDomain {
        dom = Some(g.GetDomain(storage.as_ref())?);
        let adapter = VersionPdAdapter(pd_meta);
        // Domain 场景额外校验 DDL 兼容版本。
        if let Err(err) = CheckClusterVersion(&adapter, &CheckVersionForDDL) {
            return Err(
                Annotate(Some(err), "unable to check cluster version for ddl").expect("annotate"),
            );
        }
    }

    let keyspace_id = keyspace_id_for_gc(storage.as_ref());
    let lifecycle = (deps.new_lifecycle)(dom.clone(), storage.clone());
    Ok(Mgr {
        pd: controller,
        dom,
        storage: Some(storage),
        ownsStorage: g.OwnsStorage(),
        gc_keyspace_id: keyspace_id,
        storeManager: Some((deps.new_store_manager)(tls_present)),
        gcManager: Some((deps.new_gc)(keyspace_id)),
        lifecycle: Some(lifecycle),
        closed: Mutex::new(false),
    })
}

/// 拼接 host:port；IPv6 主机名加方括号，避免与端口分隔符混淆。
fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// HandleTiKVAddress returns the TiKV status HTTP address used to fetch configs.
/// 解析 status_address；缺省补 http(s) 前缀。
/// 若 status 主机名与 node address 不一致（常见于容器网络），改用 node 主机 + status 端口。
/// 无 status_address 时直接报错。
pub fn HandleTiKVAddress(store: &Store, http_prefix: &str) -> Result<StatusUrl, SharedError> {
    let mut status_addr = store.status_address.clone();
    if status_addr.is_empty() {
        return Err(Errorf(
            "TiKV store %d does not have status address",
            &[ErrorArg::Unsigned(store.id as u128)],
        ));
    }
    let mut node_addr = store.address.clone();
    if !status_addr.starts_with("http") {
        status_addr = format!("{http_prefix}{status_addr}");
    }
    if !node_addr.starts_with("http") {
        node_addr = format!("{http_prefix}{node_addr}");
    }

    let status_url = StatusUrl::parse(&status_addr)?;
    let node_url = StatusUrl::parse(&node_addr)?;

    let mut addr = status_url.clone();
    // 主机名不一致时纠正，保留 status 端口。
    if status_url.hostname() != node_url.hostname() {
        addr.set_host(join_host_port(node_url.hostname(), status_url.port()));
    }
    Ok(addr)
}

/// Go 风格小写导出别名：返回字符串形式的 status URL。
pub fn handleTiKVAddress(store: &Store, httpPrefix: &str) -> Result<String, SharedError> {
    Ok(HandleTiKVAddress(store, httpPrefix)?.to_string())
}

/// 对每个 Up 的 TiKV（跳过 TiFlash）GET `/config`，成功则回调 `fn_`。
/// 单节点最多重试 8 次；上下文取消立即返回 Canceled。
/// 任一节点最终仍失败则中止并返回该错误。
pub fn GetConfigFromTiKV(
    ctx: &dyn CancelContext,
    pd: &dyn StoreMeta,
    client: &dyn HttpClient,
    httpPrefix: &str,
    fn_: &mut dyn FnMut(&HttpResponse) -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    let all_stores = GetAllTiKVStoresWithRetry(ctx, pd, StoreBehavior::SkipTiFlash)?;
    for store in &all_stores {
        // 仅查询处于 Up 的节点。
        if store.state != StoreState::Up {
            continue;
        }
        // 外层循环亦检查取消，避免长时间遍历。
        if ctx.is_cancelled() {
            return Err(SharedError::new(Canceled));
        }
        let addr = HandleTiKVAddress(store, httpPrefix)?;
        // 目标路径固定为 status 服务的 /config。
        let config_addr = addr.join_path("config");
        // Retry a few times like Go util.GetConfigFromTiKVStores + aggressive backoff.
        let mut last_err = None;
        for _ in 0..8 {
            if ctx.is_cancelled() {
                return Err(SharedError::new(Canceled));
            }
            match client.Get(&config_addr) {
                Ok(resp) => {
                    // 回调失败视为整体失败（不再重试该节点）。
                    fn_(&resp)?;
                    last_err = None;
                    break;
                }
                Err(e) => {
                    // 网络类错误短暂休眠后重试。
                    last_err = Some(e);
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        // 该节点用尽重试仍失败则中止。
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(())
}

/// 解析 `import.num-threads`；失败包装为 SharedError。
fn parse_import_threads_from_config(resp: &[u8]) -> Result<u32, SharedError> {
    #[derive(serde::Deserialize, Default)]
    struct Importer {
        #[serde(rename = "num-threads", default)]
        threads: u32,
    }
    #[derive(serde::Deserialize, Default)]
    struct Config {
        #[serde(rename = "import", default)]
        import: Importer,
    }
    let config: Config = serde_json::from_slice(resp)
        .map_err(|e| astersql_errors::New(format!("parse import threads: {e}")))?;
    Ok(config.import.threads)
}

/// 解析 region-split-size/keys；尺寸经 `ram_in_bytes` 转为字节。
fn parse_merge_region_size_from_config(resp: &[u8]) -> Result<(u64, u64), SharedError> {
    #[derive(serde::Deserialize, Default)]
    struct Coprocessor {
        #[serde(rename = "region-split-size", default)]
        region_split_size: String,
        #[serde(rename = "region-split-keys", default)]
        region_split_keys: u64,
    }
    #[derive(serde::Deserialize, Default)]
    struct Config {
        #[serde(rename = "coprocessor", default)]
        cop: Coprocessor,
    }
    let config: Config = serde_json::from_slice(resp)
        .map_err(|e| astersql_errors::New(format!("parse merge region: {e}")))?;
    let size = ram_in_bytes(&config.cop.region_split_size)?;
    Ok((size, config.cop.region_split_keys))
}

/// 解析 `log-backup.enable`；缺省 false。
fn parse_log_backup_enable_from_config(resp: &[u8]) -> Result<bool, SharedError> {
    #[derive(serde::Deserialize, Default)]
    struct LogBackup {
        #[serde(rename = "enable", default)]
        enable: bool,
    }
    #[derive(serde::Deserialize, Default)]
    struct Config {
        #[serde(rename = "log-backup", default)]
        log_backup: LogBackup,
    }
    let config: Config = serde_json::from_slice(resp)
        .map_err(|e| astersql_errors::New(format!("parse log-backup: {e}")))?;
    Ok(config.log_backup.enable)
}

/// 本地 RAM 尺寸解析：空串视为 0；按 1024 进制处理 K/M/G/T/P（可带 iB/B）。
fn ram_in_bytes(size: &str) -> Result<u64, SharedError> {
    let trimmed = size.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    let split = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(trimmed.len());
    let (number, suffix) = trimmed.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|e| astersql_errors::New(format!("invalid size '{size}': {e}")))?;
    let unit = suffix.trim().trim_end_matches("iB").trim_end_matches('B');
    // 单位幂次与 go-units 二进制倍数一致。
    let power = match unit.to_ascii_lowercase().as_str() {
        "" => 0,
        "k" => 1,
        "m" => 2,
        "g" => 3,
        "t" => 4,
        "p" => 5,
        _ => {
            // 未知后缀（如 XB）直接失败。
            return Err(astersql_errors::New(format!("invalid suffix: '{suffix}'")));
        }
    };
    Ok((value * 1024f64.powi(power)) as u64)
}

/// Unit helpers matching docker/go-units used by Go tests.
/// 对外暴露 MiB/GiB 常量，便于测试与 Go units 对照。
pub mod units {
    pub const MiB: u64 = super::MiB;
    pub const GiB: u64 = super::GiB;
}

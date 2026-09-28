// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Local stand-ins for PD/TiKV/gRPC/glue/utils boundaries (darwin-safe; no kvproto/grpcio).
//!
//! 模块职责：为 `restore/data` 提供可在无 kvproto/grpcio 环境下编译的本地桩，
//! 覆盖错误码、Context、metapb/recovpb、进度、连接、PD/TiKV/flashback、
//! 退避重试与简易 worker pool。对应 Go 侧对 PD/TiKV/gRPC/glue/utils 的依赖边界。
//! 约束：这里是占位能力，不是生产实现；不得把 Mem* 桩描述为已接通真实集群。
//! 测试通过注入 ClientFactory / 错误字段驱动行为，生产路径需替换真实客户端。
//! 子模块：`berrors`/`metapb`/`recovpb`/`log` 均为本地定义，不链真实 proto。
//! `WithRetry`/`WithRetryV2` 在 delay=0 时退化为有限次立即重试，适合单测。
//! `ErrorGroup` 用线程而非异步运行时，避免在 darwin 单测引入额外 runtime。
//! 若将来接入真实 grpcio，应替换 ClientFactory/Conn/流 trait 的实现而非改调用方。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mirrors `br/pkg/common.MaxStoreConcurrency`.
/// 单 store 恢复并发上限；ErrorGroup 默认 limit 与此对齐。
pub const MaxStoreConcurrency: usize = 128;

// gRPC 退避上限占位；本桩 WithRetry 默认 delay 多为 0，真实链路再接 utils。
pub const gRPCBackOffMaxDelay: Duration = Duration::from_secs(3);

// 本包统一 Result，错误类型为下方轻量 Error（非 anyhow/thiserror）。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 轻量错误：msg + 可选 BR 错误码 + 可选恢复阶段，供 isRetryErr/atStage 使用。
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
    pub stage: Option<i32>,
}

impl Error {
    /// 仅消息、无 code/stage 的基础错误。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
            stage: None,
        }
    }

    /// 带稳定错误码，对齐 Go `br/pkg/errors` 的分类字符串。
    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
            stage: None,
        }
    }

    /// Go `errors.Trace` 占位：当前原样返回，不附加栈。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// 在消息前附加上下文，保留原 code/stage，对齐 Go Annotate。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
            code: err.code,
            stage: err.stage,
        }
    }

    /// 格式化 Annotate 的简化入口（本桩不解析格式串参数）。
    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self::Annotate(err, ctx)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Display 只暴露 msg，code/stage 留给结构化检查。
        f.write_str(&self.msg)
    }
}

// 接入标准 Error trait，便于 ? 与第三方桥接。
impl std::error::Error for Error {}

/// BR error codes used by recover (mirrors `br/pkg/errors`).
/// 仅覆盖 recover 路径用到的码；文案需与 Go 侧可匹配以便测试断言。
pub mod berrors {
    use super::Error;

    /// 键空间不连续/非法范围。
    pub fn ErrInvalidRange() -> Error {
        Error::with_code("BR:Common:ErrInvalidRange", "invalid restore range")
    }

    /// 有效 peer 集合中出现 tombstone 等非法 peer。
    pub fn ErrRestoreInvalidPeer() -> Error {
        Error::with_code("BR:EBS:ErrRestoreInvalidPeer", "restore met a invalid peer")
    }

    /// region 没有任何 peer，无法选主或恢复。
    pub fn ErrRestoreRegionWithoutPeer() -> Error {
        Error::with_code(
            "BR:EBS:ErrRestoreRegionWithoutPeer",
            "restore met a region without any peer",
        )
    }
}

/// Cancellation token approximating Go `context.Context`.
/// 仅建模取消错误存储；无 deadline/value 传递，够恢复路径 cancel-on-drop。
#[derive(Clone)]
pub struct Context {
    state: Arc<ContextState>,
}

struct ContextState {
    cancelled: Mutex<Option<Error>>,
    parent: Option<Arc<ContextState>>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            state: Arc::new(ContextState {
                cancelled: Mutex::new(None),
                parent: None,
            }),
        }
    }
}

impl Context {
    /// 永不等待取消的根 Context。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 返回 (child, cancel_handle)；子节点观察父取消，但取消子节点不影响父节点。
    pub fn WithCancel(parent: &Context) -> (Self, Self) {
        let child = Self {
            state: Arc::new(ContextState {
                cancelled: Mutex::new(None),
                parent: Some(parent.state.clone()),
            }),
        };
        (child.clone(), child)
    }

    /// 写入取消原因；后续 Err/Done 可见。
    pub fn cancel(&self, err: Error) {
        *self.state.cancelled.lock().unwrap() = Some(err);
    }

    /// 若已取消则返回原因，否则 None。
    pub fn Err(&self) -> Option<Error> {
        let mut state = Some(self.state.clone());
        while let Some(current) = state {
            if let Some(err) = current.cancelled.lock().unwrap().clone() {
                return Some(err);
            }
            state = current.parent.clone();
        }
        None
    }

    /// 是否已取消（对应 Go ctx.Done 可读性）。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

/// PD metapb 最小子集：Store/Label，供地址解析与标签过滤测试。
pub mod metapb {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// store 标签键值，常见 engine=tikv|tiflash。
    pub struct StoreLabel {
        pub Key: String,
        pub Value: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 在线 store 描述；Address 供 ClientFactory 拨号。
    pub struct Store {
        pub Id: u64,
        pub Address: String,
        pub Labels: Vec<StoreLabel>,
    }

    impl Store {
        /// 对齐 Go protobuf getter 命名，便于机械对照。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
        /// 返回拨号地址字符串拷贝。
        pub fn GetAddress(&self) -> String {
            self.Address.clone()
        }
    }
}

/// recover 相关 protobuf 消息的本地结构体，字段名保持 Go 导出风格。
pub mod recovpb {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 单 peer 视角的 region 元数据：raft 进度 + 键范围 + tombstone。
    pub struct RegionMeta {
        pub RegionId: u64,
        pub PeerId: u64,
        pub LastLogTerm: u64,
        pub LastIndex: u64,
        pub CommitIndex: u64,
        pub Version: u64,
        pub Tombstone: bool,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }

    impl RegionMeta {
        /// getter 供 SortRecoverRegions 比较链调用，语义同字段本身。
        pub fn GetLastLogTerm(&self) -> u64 {
            self.LastLogTerm
        }
        /// last index getter。
        pub fn GetLastIndex(&self) -> u64 {
            self.LastIndex
        }
        /// commit index getter。
        pub fn GetCommitIndex(&self) -> u64 {
            self.CommitIndex
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 按 StoreId 拉取该 store 上全部 RegionMeta。
    pub struct ReadRegionMetaRequest {
        pub StoreId: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 指示某 region 是否以 leader/tombstone 身份恢复。
    pub struct RecoverRegionRequest {
        pub RegionId: u64,
        pub AsLeader: bool,
        pub Tombstone: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// 流关闭后的简响应，带回处理方 StoreId。
    pub struct RecoverRegionResponse {
        pub StoreId: u64,
    }

    impl RecoverRegionResponse {
        /// 响应对应的 store。
        pub fn GetStoreId(&self) -> u64 {
            self.StoreId
        }
    }
}

/// Glue progress stand-in.
/// 进度条接口：恢复各阶段 Inc，供 CLI/测试观察推进。
pub trait Progress: Send + Sync {
    fn Inc(&self);
    /// 默认按次 Inc；真实 glue 可覆写为批量更新。
    fn IncBy(&self, n: i64) {
        // 无批量原语时退化为循环 Inc，可能偏慢但语义正确。
        for _ in 0..n {
            self.Inc();
        }
    }
}

#[derive(Default)]
/// 内存进度：AtomicU64 计数，无 UI 输出。
pub struct MemProgress {
    pub current: AtomicU64,
    pub closed: AtomicBool,
}

impl MemProgress {
    pub fn new() -> Self {
        Self::default()
    }
    /// 当前已 Inc 次数，测试用来断言阶段是否走过。
    pub fn current(&self) -> u64 {
        self.current.load(Ordering::SeqCst)
    }
}

impl Progress for MemProgress {
    fn Inc(&self) {
        self.current.fetch_add(1, Ordering::SeqCst);
    }
}

/// Closeable gRPC connection stand-in.
/// 拨号返回的连接必须 Close，避免泄漏；MemConn 用原子标志记录。
pub trait Conn: Send {
    fn Close(&mut self);
}

#[derive(Default)]
/// 内存连接：Close 只翻 closed 标志，无真实 socket。
pub struct MemConn {
    pub closed: Arc<AtomicBool>,
}

impl MemConn {
    pub fn new() -> Self {
        Self {
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Conn for MemConn {
    fn Close(&mut self) {
        // 置位即可；重复 Close 保持幂等。
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// 服务端流：逐条 Recv RegionMeta，结束以 eof() 表示。
pub trait RegionMetaStream: Send {
    fn Recv(&mut self) -> Result<recovpb::RegionMeta>;
}

/// 客户端流：Send 恢复指令后 CloseAndRecv 取响应。
pub trait RecoverRegionStream: Send {
    fn Send(&mut self, req: &recovpb::RecoverRegionRequest) -> Result<()>;
    fn CloseAndRecv(&mut self) -> Result<recovpb::RecoverRegionResponse>;
}

/// TiKV recover-data RPC 客户端面：读元数据 + 开恢复流。
/// 真实实现接 gRPC；测试注入 Mock 实现。
pub trait RecoverDataClient: Send {
    fn ReadRegionMeta(
        &mut self,
        ctx: &Context,
        req: &recovpb::ReadRegionMetaRequest,
    ) -> Result<Box<dyn RegionMetaStream>>;
    fn RecoverRegion(&mut self, ctx: &Context) -> Result<Box<dyn RecoverRegionStream>>;
}

/// 按地址创建 (client, conn) 的工厂；Mgr 持有以便恢复各阶段拨号。
pub type ClientFactory = Arc<
    dyn Fn(&Context, &str) -> Result<(Box<dyn RecoverDataClient>, Box<dyn Conn>)> + Send + Sync,
>;

/// PD client stand-in for storewatch.
/// 仅 GetAllStores；不覆盖 etcd/TSO 等真实 PD 能力。
pub trait PDClient: Send + Sync {
    fn GetAllStores(&self, ctx: &Context) -> Result<Vec<metapb::Store>>;
}

#[derive(Default)]
/// 内存 PD：stores 列表可测前注入。
pub struct MemPDClient {
    pub stores: Mutex<Vec<metapb::Store>>,
}

impl PDClient for MemPDClient {
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        // 返回快照拷贝，避免锁外持有引用。
        Ok(self.stores.lock().unwrap().clone())
    }
}

/// TiKV storage / range-task flashback stand-in.
/// TikvStorage 在此几乎空接口，仅占 GetStorage 返回类型。
pub trait TikvStorage: Send + Sync {
    fn name(&self) -> &str {
        "tikv"
    }
}

#[derive(Default)]
/// 空存储桩，满足 TikvStorage trait。
pub struct MemStorage;

impl TikvStorage for MemStorage {}

#[derive(Clone, Debug, Default)]
/// flashback RPC 作用的键范围。
pub struct KeyRange {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
/// 范围任务统计：已完成 region 数，供进度/完成判定。
pub struct TaskStat {
    pub CompletedRegions: i32,
}

/// prepare/flashback 两阶段 RPC 面；真实实现打 TiKV range task。
pub trait FlashbackRpc: Send + Sync {
    fn SendPrepareFlashbackToVersionRPC(
        &self,
        ctx: &Context,
        resolve_ts: u64,
        start_ts: u64,
        r: &KeyRange,
    ) -> Result<TaskStat>;
    fn SendFlashbackToVersionRPC(
        &self,
        ctx: &Context,
        resolve_ts: u64,
        start_ts: u64,
        commit_ts: u64,
        r: &KeyRange,
    ) -> Result<TaskStat>;
}

#[derive(Default)]
/// 可注入错误与调用计数的 flashback 桩，便于测失败/次数。
pub struct MemFlashback {
    pub prepare_calls: AtomicU64,
    pub flashback_calls: AtomicU64,
    pub prepare_err: Mutex<Option<Error>>,
    pub flashback_err: Mutex<Option<Error>>,
    pub completed_regions: AtomicU64,
    pub last_flashback_start_ts: AtomicU64,
}

// prepare/flashback：先看注入错误，再递增计数并返回 CompletedRegions。
impl FlashbackRpc for MemFlashback {
    fn SendPrepareFlashbackToVersionRPC(
        &self,
        _ctx: &Context,
        _resolve_ts: u64,
        _start_ts: u64,
        _r: &KeyRange,
    ) -> Result<TaskStat> {
        // 先计数再查注入错误，保证失败路径也能被断言调用次数。
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(err) = self.prepare_err.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(TaskStat {
            CompletedRegions: self.completed_regions.load(Ordering::SeqCst) as i32,
        })
    }

    fn SendFlashbackToVersionRPC(
        &self,
        _ctx: &Context,
        _resolve_ts: u64,
        start_ts: u64,
        _commit_ts: u64,
        _r: &KeyRange,
    ) -> Result<TaskStat> {
        // 与 prepare 对称：计数与错误注入顺序一致。
        self.flashback_calls.fetch_add(1, Ordering::SeqCst);
        self.last_flashback_start_ts
            .store(start_ts, Ordering::SeqCst);
        if let Some(err) = self.flashback_err.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(TaskStat {
            CompletedRegions: self.completed_regions.load(Ordering::SeqCst) as i32,
        })
    }
}

/// Conn manager stand-in (`br/pkg/conn.Mgr` surface used here).
/// 恢复管理器门面：分配 ID、PD/存储/flashback/工厂；对齐 Go conn.Mgr 最小面。
/// 恢复管理器门面：分配 ID、PD/存储/flashback/工厂访问。
/// 对齐 Go glue/manager 在 recover-data 中的最小依赖面。
pub trait Mgr: Send + Sync {
    fn RecoverBaseAllocID(&self, ctx: &Context, max_alloc_id: u64) -> Result<()>;
    fn PDClient(&self) -> Arc<dyn PDClient>;
    fn GetStorage(&self) -> Arc<dyn TikvStorage>;
    fn GetFlashback(&self) -> Arc<dyn FlashbackRpc>;
    /// 未注入时为 None，调用方需处理缺失工厂。
    fn ClientFactory(&self) -> Option<ClientFactory> {
        None
    }
}

#[derive(Default)]
/// 内存 Mgr：字段均可测前配置；ClientFactory 可选。
pub struct MemMgr {
    pub max_alloc_id: Mutex<u64>,
    pub recover_alloc_err: Mutex<Option<Error>>,
    pub pd: Arc<MemPDClient>,
    pub storage: Arc<MemStorage>,
    pub flashback: Arc<MemFlashback>,
    pub client_factory: Mutex<Option<ClientFactory>>,
}

// new 组装默认 MemPD/Storage/Flashback；set_client_factory 供测试注入。
impl MemMgr {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入拨号工厂；未设置时生产路径应失败或走默认（本桩无默认拨号）。
    pub fn set_client_factory(&self, f: ClientFactory) {
        *self.client_factory.lock().unwrap() = Some(f);
    }
}

// RecoverBaseAllocID 可注入失败；其余 getter 返回内部 Arc。
impl Mgr for MemMgr {
    /// 把计划得出的 MaxAllocID 写入，供 PD 后续分配避让。
    fn RecoverBaseAllocID(&self, _ctx: &Context, max_alloc_id: u64) -> Result<()> {
        // 测试可预设 recover_alloc_err 强制分配失败。
        if let Some(err) = self.recover_alloc_err.lock().unwrap().clone() {
            return Err(err);
        }
        // 成功路径记录最大已分配 ID。
        *self.max_alloc_id.lock().unwrap() = max_alloc_id;
        Ok(())
    }

    /// 返回 PD 客户端桩。
    fn PDClient(&self) -> Arc<dyn PDClient> {
        self.pd.clone()
    }

    /// 返回存储句柄；本桩为 MemStorage。
    fn GetStorage(&self) -> Arc<dyn TikvStorage> {
        self.storage.clone()
    }

    /// 返回 flashback RPC 实现。
    fn GetFlashback(&self) -> Arc<dyn FlashbackRpc> {
        self.flashback.clone()
    }

    fn ClientFactory(&self) -> Option<ClientFactory> {
        self.client_factory.lock().unwrap().clone()
    }
}

/// Minimal storewatch callback / watcher stand-ins.
/// 回调可选；StoreWatcher::Step 只检测“新 store”，reboot 需显式 notify。
pub struct StoreWatchCallback {
    pub on_reboot: Option<Box<dyn Fn(&metapb::Store) + Send + Sync>>,
    pub on_disconnect: Option<Box<dyn Fn(&metapb::Store) + Send + Sync>>,
    pub on_new: Option<Box<dyn Fn(&metapb::Store) + Send + Sync>>,
}

/// 组装 StoreWatchCallback，参数顺序对齐 Go MakeCallback。
pub fn MakeCallback(
    on_reboot: Option<Box<dyn Fn(&metapb::Store) + Send + Sync>>,
    on_disconnect: Option<Box<dyn Fn(&metapb::Store) + Send + Sync>>,
    on_new: Option<Box<dyn Fn(&metapb::Store) + Send + Sync>>,
) -> StoreWatchCallback {
    StoreWatchCallback {
        on_reboot,
        on_disconnect,
        on_new,
    }
}

/// 轮询 PD GetAllStores，对未见过的 Id 触发 on_new。
pub struct StoreWatcher {
    pd: Arc<dyn PDClient>,
    cb: StoreWatchCallback,
    known: Mutex<HashMap<u64, metapb::Store>>,
}

impl StoreWatcher {
    /// 绑定 PD 与回调；known 初始为空。
    pub fn New(pd: Arc<dyn PDClient>, cb: StoreWatchCallback) -> Self {
        Self {
            pd,
            cb,
            known: Mutex::new(HashMap::new()),
        }
    }

    /// 单步轮询：仅处理新增 store，不模拟 disconnect 拓扑。
    pub fn Step(&self, ctx: &Context) -> Result<()> {
        let stores = self.pd.GetAllStores(ctx)?;
        let mut known = self.known.lock().unwrap();
        for s in stores {
            // 仅首次见到的 store 触发 on_new，模拟上线事件。
            if !known.contains_key(&s.Id) {
                if let Some(f) = &self.cb.on_new {
                    f(&s);
                }
                known.insert(s.Id, s);
            }
        }
        Ok(())
    }

    /// 测试辅助：手动触发 on_reboot。
    pub fn notify_reboot(&self, s: &metapb::Store) {
        if let Some(f) = &self.cb.on_reboot {
            f(s);
        }
    }
}

/// Backoff strategy stand-in (`utils.BackoffStrategy`).
/// RemainingAttempts 决定是否继续；零时长仅表示立即重试，与 Go time.After(0) 一致。
pub trait BackoffStrategy: Send {
    fn NextBackoff(&mut self, err: &Error) -> Duration;
    fn RemainingAttempts(&self) -> i32;
    fn Attempt(&self) -> i32 {
        0
    }
}

/// 恢复主路径退避：结合 is_retry 谓词与最大次数。
pub struct RecoveryBackoffStrategy {
    pub is_retry: Box<dyn Fn(&Error) -> bool + Send>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub delay: Duration,
}

impl RecoveryBackoffStrategy {
    /// Go recoveryMaxAttempts 为 16；测试桩用零延迟保留次数语义且不实际等待。
    pub fn new(is_retry: Box<dyn Fn(&Error) -> bool + Send>) -> Self {
        Self {
            is_retry,
            attempts: 0,
            max_attempts: 16,
            // 默认 0 延迟：单测瞬时失败/成功，无 wall-clock 等待。
            delay: Duration::from_millis(0),
        }
    }
}

// 不可重试错误清空剩余次数；可重试错误消耗一次尝试。
impl BackoffStrategy for RecoveryBackoffStrategy {
    fn NextBackoff(&mut self, err: &Error) -> Duration {
        if !(self.is_retry)(err) {
            self.attempts = self.max_attempts;
            return Duration::ZERO;
        }
        self.attempts += 1;
        self.delay
    }

    fn RemainingAttempts(&self) -> i32 {
        (self.max_attempts - self.attempts).max(0)
    }

    fn Attempt(&self) -> i32 {
        self.attempts
    }
}

/// flashback 路径退避：不区分错误类型，只看次数。
pub struct FlashBackBackoffStrategy {
    pub attempts: i32,
    pub max_attempts: i32,
    pub delay: Duration,
}

impl Default for FlashBackBackoffStrategy {
    fn default() -> Self {
        Self {
            attempts: 0,
            max_attempts: 3,
            delay: Duration::from_millis(0),
        }
    }
}

// 与 Recovery 策略类似，但忽略 is_retry，任何错误都可再试至上限。
impl BackoffStrategy for FlashBackBackoffStrategy {
    fn NextBackoff(&mut self, _err: &Error) -> Duration {
        self.attempts += 1;
        self.delay
    }

    fn RemainingAttempts(&self) -> i32 {
        (self.max_attempts - self.attempts).max(0)
    }

    fn Attempt(&self) -> i32 {
        self.attempts
    }
}

/// 装箱 RecoveryBackoffStrategy，供 WithRetryV2 使用。
pub fn NewRecoveryBackoffStrategy(
    is_retry: Box<dyn Fn(&Error) -> bool + Send>,
) -> Box<dyn BackoffStrategy> {
    Box::new(RecoveryBackoffStrategy::new(is_retry))
}

/// 装箱默认 FlashBackBackoffStrategy。
pub fn NewFlashBackBackoffStrategy() -> Box<dyn BackoffStrategy> {
    Box::new(FlashBackBackoffStrategy::default())
}

/// WithRetryV2 retries while the strategy has attempts remaining.
/// 每次失败后再查 ctx 取消，与 Go `WithRetryV2` 的调用/取消顺序一致。
pub fn WithRetryV2<T, F>(
    ctx: &Context,
    mut backoff: Box<dyn BackoffStrategy>,
    mut fn_: F,
) -> Result<T>
where
    F: FnMut(&Context) -> Result<T>,
{
    let mut errors = Vec::new();
    while backoff.RemainingAttempts() > 0 {
        match fn_(ctx) {
            Ok(v) => return Ok(v),
            Err(err) => {
                errors.push(err);
                // Go ignores the context error here and returns all operation errors.
                if ctx.Done() {
                    return Err(combine_errors(errors));
                }
                let delay = backoff.NextBackoff(errors.last().expect("just pushed retry error"));
                if wait_or_cancel(ctx, delay) {
                    return Err(combine_errors(errors));
                }
            }
        }
    }
    if errors.is_empty() {
        Err(Error::new("retry attempts exhausted"))
    } else {
        Err(combine_errors(errors))
    }
}

/// WithRetry for prepare-flashback path.
/// 与 V2 类似但闭包无返回值、签名对齐 Go prepare-flashback 调用点。
pub fn WithRetry<F>(ctx: &Context, mut fn_: F, mut backoff: Box<dyn BackoffStrategy>) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    let mut errors = Vec::new();
    while backoff.RemainingAttempts() > 0 {
        match fn_() {
            Ok(()) => return Ok(()),
            Err(err) => {
                errors.push(err);
                if ctx.Done() {
                    return Err(combine_errors(errors));
                }
                let delay = backoff.NextBackoff(errors.last().expect("just pushed retry error"));
                if wait_or_cancel(ctx, delay) {
                    return Err(combine_errors(errors));
                }
            }
        }
    }
    if errors.is_empty() {
        Err(Error::new("retry attempts exhausted"))
    } else {
        Err(combine_errors(errors))
    }
}

/// Wait for a backoff while retaining Go's ability to stop promptly on context
/// cancellation. Returns true when cancellation won the wait.
fn wait_or_cancel(ctx: &Context, delay: Duration) -> bool {
    if delay.is_zero() {
        return ctx.Done();
    }
    let started = std::time::Instant::now();
    while started.elapsed() < delay {
        if ctx.Done() {
            return true;
        }
        std::thread::sleep(
            delay
                .saturating_sub(started.elapsed())
                .min(Duration::from_millis(10)),
        );
    }
    ctx.Done()
}

fn combine_errors(errors: Vec<Error>) -> Error {
    let last = errors.last().expect("non-empty retry errors");
    Error {
        msg: errors
            .iter()
            .map(|err| err.msg.as_str())
            .collect::<Vec<_>>()
            .join("; "),
        code: last.code,
        stage: last.stage,
    }
}

/// Simple worker-pool + errgroup stand-in using a semaphore and threads.
/// 非完整 golang.org/x/sync/errgroup；用线程+槽位模拟有限并发。
/// 记录期望并发度；实际限流在 ErrorGroup.slots。
pub struct WorkerPool {
    limit: usize,
}

impl WorkerPool {
    /// limit<1 时抬到 1，避免零并发死锁。
    pub fn New(limit: usize, _name: &str) -> Self {
        Self {
            // 至少 1 个并发槽，防止 New(0) 导致永久等待。
            limit: limit.max(1),
        }
    }
}

/// 首错取消：子任务失败写入 errors 并 cancel，Wait 汇总。
pub struct ErrorGroup {
    errors: Arc<Mutex<Option<Error>>>,
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
    slots: Arc<Mutex<usize>>,
    limit: usize,
    cancelled: Context,
}

impl ErrorGroup {
    /// 从父 Context 派生子取消上下文，默认 limit=MaxStoreConcurrency。
    pub fn WithContext(ctx: &Context) -> (Self, Context) {
        let (child, cancel) = Context::WithCancel(ctx);
        (
            Self {
                errors: Arc::new(Mutex::new(None)),
                handles: Mutex::new(Vec::new()),
                slots: Arc::new(Mutex::new(0)),
                limit: MaxStoreConcurrency,
                cancelled: cancel,
            },
            child,
        )
    }

    /// 覆盖并发槽位数；0 视为 1。
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = if limit == 0 { 1 } else { limit };
        self
    }

    /// 后台执行 f：先自旋等待槽位，失败则 cancel 并记录首错。
    pub fn Go<F>(&self, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let errors = self.errors.clone();
        let slots = self.slots.clone();
        let limit = self.limit;
        let cancelled = self.cancelled.clone();
        let handle = std::thread::spawn(move || {
            loop {
                {
                    let mut g = slots.lock().unwrap();
                    // 获取并发槽；满则让出并检查取消。
                    if *g < limit {
                        *g += 1;
                        break;
                    }
                }
                // 等待槽位期间若已取消则直接退出，不再执行 f。
                if cancelled.Done() {
                    return;
                }
                std::thread::yield_now();
            }
            let result = f();
            {
                let mut g = slots.lock().unwrap();
                *g = g.saturating_sub(1);
            }
            // 首错取消整个 group，后续 Wait 能尽快结束。
            if let Err(err) = result {
                cancelled.cancel(err.clone());
                let mut eg = errors.lock().unwrap();
                // 只保留第一个错误，后续失败不覆盖。
                if eg.is_none() {
                    *eg = Some(err);
                }
            }
        });
        self.handles.lock().unwrap().push(handle);
    }

    /// join 全部任务后返回首错或取消原因。
    pub fn Wait(&self) -> Result<()> {
        let handles: Vec<_> = self.handles.lock().unwrap().drain(..).collect();
        for h in handles {
            let _ = h.join();
        }
        // 优先返回任务记录的错误，其次取消原因。
        if let Some(err) = self.errors.lock().unwrap().clone() {
            Err(err)
        } else if let Some(err) = self.cancelled.Err() {
            Err(err)
        } else {
            Ok(())
        }
    }
}

impl WorkerPool {
    /// 将任务交给 eg.Go；pool.limit 仅作意图记录。
    pub fn ApplyOnErrorGroup<F>(&self, eg: &ErrorGroup, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        // Limit is applied via eg.with_limit before Go; pool records intended concurrency.
        // 限流以 eg.limit 为准；WorkerPool.limit 仅文档/对称 Go API。
        let _ = self.limit;
        eg.Go(f);
    }
}

/// EOF sentinel for region-meta streams.
/// 流结束哨兵；is_eof 识别后不得当可重试错误。
pub fn eof() -> Error {
    Error::with_code("io.EOF", "EOF")
}

/// 判断是否为流结束；错误注解会保留稳定哨兵码，不依赖易误判的文本子串。
pub fn is_eof(err: &Error) -> bool {
    err.code == Some("io.EOF")
}

/// Logging stubs (no-op; keep call sites).
/// 保留调用点以便将来接真实日志，当前全空操作。
pub mod log {
    // 四级日志空实现，避免测试输出噪音。
    pub fn Info(_msg: &str) {}
    pub fn Warn(_msg: &str) {}
    pub fn Error(_msg: &str) {}
    pub fn Debug(_msg: &str) {}
}

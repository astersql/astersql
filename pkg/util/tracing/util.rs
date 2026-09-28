// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// TiDB tracing 核心工具：Span、Context、事件分类与 Region。
//
// 由 `util.go` 迁移。提供可回调录制的 span 树、不可变 Context 派生、
// 按位掩码的 TraceCategory 开关，以及将 span / begin-end 事件组合的 Region。
// 术语：两阶段提交（2PC）相关类别见 `Txn2PC`；Region 此处指 tracing 区间，
// 非 TiKV 数据分片。

use std::collections::HashMap;
use std::fmt;
use std::ops::BitOr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::SystemTime;

/// Baggage 标记，用于识别 TiDB 产生的 trace。
/// Baggage marker used to identify TiDB traces.
pub const TiDBTrace: &str = "tr";

/// 已完成 span 的原始快照，交给 [`CallbackRecorder`] 回调。
/// The completed-span representation delivered to a [`CallbackRecorder`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawSpan {
    pub operation: String,
    pub span_id: u64,
    pub parent_span_id: u64,
    pub baggage: HashMap<String, String>,
    /// 按记录顺序保留的结构化日志键值。
    pub logs: Vec<(String, String)>,
}

/// 录制器：收到已完成 span 后立刻调用回调。
/// A recorder which immediately invokes the supplied callback.
pub struct CallbackRecorder<F>
where
    F: Fn(RawSpan),
{
    callback: F,
}

impl<F> CallbackRecorder<F>
where
    F: Fn(RawSpan),
{
    /// 用给定回调构造录制器。
    pub fn new(callback: F) -> Self {
        Self { callback }
    }

    /// 将 span 交给回调处理。
    pub fn RecordSpan(&self, span: RawSpan) {
        (self.callback)(span);
    }
}

/// 全局录制器类型别名：线程安全的 RawSpan 回调。
type Recorder = Arc<dyn Fn(RawSpan) + Send + Sync>;

/// 懒初始化全局录制器槽位。
fn global_recorder() -> &'static RwLock<Option<Recorder>> {
    static RECORDER: OnceLock<RwLock<Option<Recorder>>> = OnceLock::new();
    RECORDER.get_or_init(|| RwLock::new(None))
}

/// 下一个可分配的 span_id（从 1 起）。
static NEXT_SPAN_ID: AtomicU64 = AtomicU64::new(1);

/// Span 内部共享状态（经 Arc 可克隆句柄）。
struct SpanInner {
    operation: String,
    span_id: u64,
    parent_span_id: u64,
    baggage: Mutex<HashMap<String, String>>,
    logs: Mutex<Vec<(String, String)>>,
    recorder: Option<Recorder>,
    finished: AtomicBool,
    noop: bool,
}

/// 可克隆的 span 句柄；任一克隆 finish 后只录制一次。
/// A clonable span handle. Finishing any clone records the span exactly once.
#[derive(Clone)]
pub struct Span(Arc<SpanInner>);

impl Span {
    /// 构造真实 span：继承父 baggage，分配新 span_id。
    fn new(operation: &str, parent: Option<&Span>, recorder: Option<Recorder>) -> Self {
        let parent_span_id = parent.map_or(0, |span| span.span_id());
        // 子 span 继承父 baggage，便于在整棵树上传播 TiDBTrace 等标记。
        let baggage = parent
            .map(|span| span.0.baggage.lock().unwrap().clone())
            .unwrap_or_default();
        Self(Arc::new(SpanInner {
            operation: operation.to_owned(),
            span_id: NEXT_SPAN_ID.fetch_add(1, Ordering::Relaxed),
            parent_span_id,
            baggage: Mutex::new(baggage),
            logs: Mutex::new(Vec::new()),
            recorder,
            finished: AtomicBool::new(false),
            noop: false,
        }))
    }

    /// 构造无操作 span：不录制、span_id 为 0。
    fn noop() -> Self {
        Self(Arc::new(SpanInner {
            operation: "DefaultSpan".to_owned(),
            span_id: 0,
            parent_span_id: 0,
            baggage: Mutex::new(HashMap::new()),
            logs: Mutex::new(Vec::new()),
            recorder: None,
            finished: AtomicBool::new(false),
            noop: true,
        }))
    }

    /// 结束 span：noop 或已结束则忽略；否则回调录制 RawSpan。
    pub fn finish(&self) {
        // swap 保证多克隆并发 finish 时只有一次真正录制。
        if self.0.noop || self.0.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(recorder) = &self.0.recorder {
            recorder(RawSpan {
                operation: self.0.operation.clone(),
                span_id: self.0.span_id,
                parent_span_id: self.0.parent_span_id,
                baggage: self.0.baggage.lock().unwrap().clone(),
                logs: self.0.logs.lock().unwrap().clone(),
            });
        }
    }

    /// 是否为无操作 span。
    pub fn is_noop(&self) -> bool {
        self.0.noop
    }

    /// 当前 span 的 ID。
    pub fn span_id(&self) -> u64 {
        self.0.span_id
    }

    /// 父 span 的 ID；根为 0。
    pub fn parent_span_id(&self) -> u64 {
        self.0.parent_span_id
    }

    /// 写入 baggage 键值。
    pub fn set_baggage_item(&self, key: &str, value: &str) {
        self.0
            .baggage
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_owned());
    }

    /// 读取 baggage 项。
    pub fn baggage_item(&self, key: &str) -> Option<String> {
        self.0.baggage.lock().unwrap().get(key).cloned()
    }

    /// 记录一组结构化日志键值；noop span 与 Go `noopSpan.LogKV` 一样直接丢弃。
    pub fn log_kv(&self, key: &str, value: &str) {
        if self.0.noop {
            return;
        }
        self.0
            .logs
            .lock()
            .unwrap()
            .push((key.to_owned(), value.to_owned()));
    }

    /// 创建子 span，复用同一录制器。
    fn child(&self, operation: &str) -> Self {
        Self::new(operation, Some(self), self.0.recorder.clone())
    }
}

/// SQL 语句级 trace 关联信息（会话别名、TraceID、连接 ID）。
/// Information associated with a SQL statement trace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceInfo {
    pub SessionAlias: String,
    pub TraceID: Vec<u8>,
    pub ConnectionID: u64,
}

/// Context 内部可变字段的快照数据。
#[derive(Clone, Default)]
struct ContextData {
    span: Option<Span>,
    sink: Option<Arc<dyn Sink>>,
    trace_id: Vec<u8>,
    trace_info: Option<Arc<TraceInfo>>,
}

/// 不可变 tracing 上下文；派生时保留既有字段并替换目标项。
/// Immutable tracing context. Deriving a value preserves all existing fields.
#[derive(Clone, Default)]
pub struct Context(Arc<ContextData>);

impl Context {
    /// 空后台上下文（无 span / sink / TraceInfo）。
    pub fn background() -> Self {
        Self::default()
    }

    /// 派生带指定 span 的新 Context。
    pub fn with_span(&self, span: Span) -> Self {
        let mut data = (*self.0).clone();
        data.span = Some(span);
        Self(Arc::new(data))
    }

    /// 派生带指定 TraceID 的新 Context。
    pub fn with_trace_id(&self, trace_id: Vec<u8>) -> Self {
        let mut data = (*self.0).clone();
        data.trace_id = trace_id;
        Self(Arc::new(data))
    }

    /// 两个 Context 是否共享同一内部 Arc（未派生）。
    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// 创建根 span，并把回调安装为全局录制器；同时写入 TiDBTrace baggage。
/// Creates a root span and installs its callback as the global recorder.
pub fn NewRecordedTrace<F>(opName: &str, callback: F) -> Span
where
    F: Fn(RawSpan) + Send + Sync + 'static,
{
    let recorder: Recorder = Arc::new(callback);
    *global_recorder().write().unwrap() = Some(recorder.clone());
    let span = Span::new(opName, None, Some(recorder));
    span.set_baggage_item(TiDBTrace, "1");
    span
}

/// 用全局录制器开根 span；无录制器则返回 noop。
fn start_global_span(operation: &str) -> Span {
    match global_recorder().read().unwrap().clone() {
        Some(recorder) => Span::new(operation, None, Some(recorder)),
        None => Span::noop(),
    }
}

/// 从 Context 取 span；缺失时返回 noop。
pub fn SpanFromContext(ctx: &Context) -> Span {
    ctx.0.span.clone().unwrap_or_else(Span::noop)
}

/// 在 Context 的真实父 span 下建子 span；否则返回 noop 且不改 Context。
/// Returns a child of the context span or a no-op span when no real parent exists.
pub fn ChildSpanFromContxt(ctx: Context, opName: &str) -> (Span, Context) {
    if let Some(parent) = &ctx.0.span {
        if !parent.is_noop() {
            let child = parent.child(opName);
            let next = ctx.with_span(child.clone());
            return (child, next);
        }
    }
    (Span::noop(), ctx)
}

/// 事件接收端：把结构化 Event 写入外部存储或日志。
pub trait Sink: Send + Sync {
    fn record(&self, ctx: &Context, event: Event);
}

/// 飞行记录器：扩展 Sink，用于会话级事件缓冲。
pub trait FlightRecorder: Sink {}

/// 将 FlightRecorder 挂到 Context。
pub fn WithFlightRecorder<T>(ctx: Context, sink: Arc<T>) -> Context
where
    T: FlightRecorder + 'static,
{
    let mut data = (*ctx.0).clone();
    data.sink = Some(sink);
    Context(Arc::new(data))
}

/// 取出 Context 上的 Sink。
pub fn GetSink(ctx: &Context) -> Option<Arc<dyn Sink>> {
    ctx.0.sink.clone()
}

/// 取出 Context 上的 TraceID 字节。
pub fn ExtractTraceID(ctx: &Context) -> Vec<u8> {
    ctx.0.trace_id.clone()
}

/// 追踪事件类别的原子位掩码（每位一类）。
/// Atomic bit mask for tracing event categories.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TraceCategory(pub u64);

/// 事务生命周期事件。
pub const TxnLifecycle: TraceCategory = TraceCategory(1 << 0);
/// 两阶段提交（2PC）相关事件。
pub const Txn2PC: TraceCategory = TraceCategory(1 << 1);
/// 事务锁解析事件。
pub const TxnLockResolve: TraceCategory = TraceCategory(1 << 2);
/// 语句生命周期事件。
pub const StmtLifecycle: TraceCategory = TraceCategory(1 << 3);
/// 语句执行计划事件。
pub const StmtPlan: TraceCategory = TraceCategory(1 << 4);
/// KV 请求事件。
pub const KvRequest: TraceCategory = TraceCategory(1 << 5);
/// 未知客户端事件。
pub const UnknownClient: TraceCategory = TraceCategory(1 << 6);
/// 通用类别（Region begin/end 等）。
pub const General: TraceCategory = TraceCategory(1 << 7);
/// DDL Job 事件。
pub const DDLJob: TraceCategory = TraceCategory(1 << 8);
/// 开发调试事件。
pub const DevDebug: TraceCategory = TraceCategory(1 << 9);
/// TiKV 请求事件。
pub const TiKVRequest: TraceCategory = TraceCategory(1 << 10);
/// TiKV 写细节事件。
pub const TiKVWriteDetails: TraceCategory = TraceCategory(1 << 11);
/// TiKV 读细节事件。
pub const TiKVReadDetails: TraceCategory = TraceCategory(1 << 12);
/// Region Cache（TiKV 分片路由缓存）事件。
pub const RegionCache: TraceCategory = TraceCategory(1 << 13);
/// 已知类别哨兵：下一未用位，用于计算 AllCategories。
const traceCategorySentinel: u64 = 1 << 14;
/// 全部已知类别的位或掩码。
pub const AllCategories: TraceCategory = TraceCategory(traceCategorySentinel - 1);

impl TraceCategory {
    /// 空掩码：未启用任何类别。
    pub const NONE: Self = Self(0);
    /// 全部具名单类别常量列表。
    pub const KNOWN: [Self; 14] = [
        TxnLifecycle,
        Txn2PC,
        TxnLockResolve,
        StmtLifecycle,
        StmtPlan,
        KvRequest,
        UnknownClient,
        General,
        DDLJob,
        DevDebug,
        TiKVRequest,
        TiKVWriteDetails,
        TiKVReadDetails,
        RegionCache,
    ];

    /// 单类别对应的稳定字符串名；组合/未知返回空串。
    pub fn as_str(self) -> &'static str {
        match self.0 {
            1 => "txn_lifecycle",
            2 => "txn_2pc",
            4 => "txn_lock_resolve",
            8 => "stmt_lifecycle",
            16 => "stmt_plan",
            32 => "kv_request",
            64 => "unknown_client",
            128 => "general",
            256 => "ddl_job",
            512 => "dev_debug",
            1024 => "tikv_request",
            2048 => "tikv_write_details",
            4096 => "tikv_read_details",
            8192 => "region_cache",
            _ => "",
        }
    }

    /// 对应 Go 的 String()：委托 Display。
    pub fn String(self) -> String {
        self.to_string()
    }
}

impl BitOr for TraceCategory {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl fmt::Display for TraceCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.as_str();
        if name.is_empty() {
            write!(formatter, "unknown({})", self.0)
        } else {
            formatter.write_str(name)
        }
    }
}

/// 进程内已启用的类别掩码。
static enabledCategories: AtomicU64 = AtomicU64::new(0);

/// 按位或启用类别。
pub fn Enable(categories: TraceCategory) {
    enabledCategories.fetch_or(categories.0, Ordering::AcqRel);
}

/// 按位清除禁用类别。
pub fn Disable(categories: TraceCategory) {
    enabledCategories.fetch_and(!categories.0, Ordering::AcqRel);
}

/// 整体替换已启用类别掩码。
pub fn SetCategories(categories: TraceCategory) {
    enabledCategories.store(categories.0, Ordering::Release);
}

/// 读取当前已启用类别。
pub fn GetEnabledCategories() -> TraceCategory {
    TraceCategory(enabledCategories.load(Ordering::Acquire))
}

/// classic 内核下非测试环境关闭 tracing 类别检查。
fn classic_kernel() -> bool {
    std::env::var("TIDB_KERNEL_TYPE").is_ok_and(|value| value.eq_ignore_ascii_case("classic"))
}

/// 指定类别是否已启用（classic 内核非 test 恒为 false）。
pub fn IsEnabled(category: TraceCategory) -> bool {
    if classic_kernel() && !cfg!(test) {
        return false;
    }
    enabledCategories.load(Ordering::Acquire) & category.0 != 0
}

/// 按名称解析类别；未知名返回 NONE。
pub fn ParseTraceCategory(category: &str) -> TraceCategory {
    TraceCategory::KNOWN
        .into_iter()
        .find(|candidate| candidate.as_str() == category)
        .unwrap_or(TraceCategory::NONE)
}

/// Chrome Trace Event 风格的相位标记。
pub type Phase = &'static str;
/// Begin。
pub const PhaseBegin: Phase = "B";
/// End。
pub const PhaseEnd: Phase = "E";
/// Async Begin。
pub const PhaseAsyncBegin: Phase = "b";
/// Async End。
pub const PhaseAsyncEnd: Phase = "e";
/// Flow Begin。
pub const PhaseFlowBegin: Phase = "s";
/// Flow End。
pub const PhaseFlowEnd: Phase = "f";
/// Instant。
pub const PhaseInstant: Phase = "i";

/// 结构化事件字段，对应 Go zap field。
/// A structured event field corresponding to a Go zap field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventField {
    pub key: String,
    pub value: String,
}

/// 一条结构化 tracing 事件。
#[derive(Clone, Debug)]
pub struct Event {
    pub Timestamp: SystemTime,
    pub Name: String,
    pub Phase: Phase,
    pub TraceID: Vec<u8>,
    pub Fields: Vec<EventField>,
    pub Category: TraceCategory,
}

/// Region：组合 runtime span、回调 Span 与 begin/end 事件。
/// A region combines a `tracing` span, the callback span and begin/end events.
pub struct Region {
    runtime_span: Option<tracing::span::EnteredSpan>,
    pub Span: Option<Span>,
    event: Option<Event>,
    sink: Option<Arc<dyn Sink>>,
    ctx: Option<Context>,
}

/// 进入与 Go `runtime/trace.StartRegion` 对应的运行时区间。
fn start_runtime_region(regionType: &str) -> tracing::span::EnteredSpan {
    tracing::span!(
        tracing::Level::TRACE,
        "tidb_region",
        region_type = regionType
    )
    .entered()
}

/// 在 Context 下启动 Region：进入 tracing span，可选建子回调 span 并写 begin 事件。
pub fn StartRegion(ctx: Context, regionType: &str) -> Region {
    let runtime_span = start_runtime_region(regionType);
    // Go 对 context 中任意非 nil span 都保留非 nil 结果；noop 父 span
    // 产生 noop 子 span，真实父 span 则产生可录制子 span。
    let span = ctx.0.span.as_ref().map(|parent| {
        if parent.is_noop() {
            Span::noop()
        } else {
            parent.child(regionType)
        }
    });

    let mut region = Region {
        runtime_span: Some(runtime_span),
        Span: span,
        event: None,
        sink: None,
        ctx: None,
    };
    // General 类别启用且存在 Sink 时，记录 PhaseBegin 事件。
    if IsEnabled(General) {
        if let Some(sink) = GetSink(&ctx) {
            let event = Event {
                Timestamp: SystemTime::now(),
                Name: regionType.to_owned(),
                Phase: PhaseBegin,
                TraceID: ExtractTraceID(&ctx),
                Fields: Vec::new(),
                Category: General,
            };
            sink.record(&ctx, event.clone());
            region.event = Some(event);
            region.sink = Some(sink);
            region.ctx = Some(ctx);
        }
    }
    region
}

/// 用全局录制器新建根 span 后启动 Region，并返回派生 Context。
pub fn StartRegionWithNewRootSpan(ctx: Context, regionType: &str) -> (Region, Context) {
    let span = start_global_span(regionType);
    let next = ctx.with_span(span.clone());
    // Go 版本直接把新根 span 放进 Region，不经 StartRegion 创建子 span，
    // 也不在这条 API 上产生 flight-recorder begin/end 事件。
    let region = Region {
        runtime_span: Some(start_runtime_region(regionType)),
        Span: Some(span),
        event: None,
        sink: None,
        ctx: None,
    };
    (region, next)
}

/// 启动 Region，并把子 span（若有）写回 Context。
pub fn StartRegionEx(ctx: Context, regionType: &str) -> (Region, Context) {
    let region = StartRegion(ctx.clone(), regionType);
    let next = match &region.Span {
        Some(span) => ctx.with_span(span.clone()),
        None => ctx,
    };
    (region, next)
}

impl Region {
    /// 结束 Region：finish 回调 span、退出 runtime span、写 PhaseEnd。
    pub fn end(&mut self) {
        if let Some(span) = self.Span.take() {
            span.finish();
        }
        self.runtime_span.take();
        if let Some(event) = self.event.as_mut() {
            event.Phase = PhaseEnd;
            event.Timestamp = SystemTime::now();
            if let (Some(sink), Some(ctx)) = (&self.sink, &self.ctx) {
                sink.record(ctx, event.clone());
            }
        }
    }

    /// Go 风格别名，委托 [`end`](Self::end)。
    pub fn End(&mut self) {
        self.end();
    }
}

/// 从 Context 取出 TraceInfo。
pub fn TraceInfoFromContext(ctx: &Context) -> Option<Arc<TraceInfo>> {
    ctx.0.trace_info.clone()
}

/// 将 TraceInfo 挂到 Context；`None` 时原样返回。
pub fn ContextWithTraceInfo(ctx: Context, info: Option<TraceInfo>) -> Context {
    let Some(info) = info else {
        return ctx;
    };
    let mut data = (*ctx.0).clone();
    data.trace_info = Some(Arc::new(info));
    Context(Arc::new(data))
}

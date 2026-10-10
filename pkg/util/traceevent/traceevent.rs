// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 结构化追踪事件核心：类别位图、Event/Field、模式开关、环形缓冲与渲染。
//
// 对应 Go `traceevent.go`。`trace_event` 在类别启用时写入 flight recorder、
// Context 上的 Trace sink 与全局 LogSink；模式 off/base/full 控制 recorder 与日志。

use crate::flightrecorder::{Trace, get_flight_recorder};
use rand::random;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 追踪模式：关闭（不记录）。
pub const MODE_OFF: &str = "off";
/// 追踪模式：仅缓冲到 recorder，不立即打日志。
pub const MODE_BASE: &str = "base";
/// 追踪模式：缓冲并立即经 LogSink 输出。
pub const MODE_FULL: &str = "full";
/// Go 风格常量别名。
pub const ModeOff: &str = MODE_OFF;
pub const ModeBase: &str = MODE_BASE;
pub const ModeFull: &str = MODE_FULL;

/// 追踪类别位图；每位对应一类事件（事务生命周期、2PC、语句计划、KV 等）。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceCategory(pub u64);

impl TraceCategory {
    /// 是否包含 `other` 中的任一位置位。
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// 类别的稳定字符串名（未知位输出 `unknown(N)`）。
    pub fn name(self) -> String {
        match self {
            TXN_LIFECYCLE => "txn_lifecycle".into(),
            TXN_2PC => "txn_2pc".into(),
            TXN_LOCK_RESOLVE => "txn_lock_resolve".into(),
            STMT_LIFECYCLE => "stmt_lifecycle".into(),
            STMT_PLAN => "stmt_plan".into(),
            KV_REQUEST => "kv_request".into(),
            UNKNOWN_CLIENT => "unknown_client".into(),
            GENERAL => "general".into(),
            DDL_JOB => "ddl_job".into(),
            DEV_DEBUG => "dev_debug".into(),
            REGION_CACHE => "region_cache".into(),
            TIKV_REQUEST => "tikv_request".into(),
            TIKV_WRITE_DETAILS => "tikv_write_details".into(),
            TIKV_READ_DETAILS => "tikv_read_details".into(),
            _ => format!("unknown({})", self.0),
        }
    }
}

impl fmt::Display for TraceCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl BitOr for TraceCategory {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}
impl BitOrAssign for TraceCategory {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
impl BitAnd for TraceCategory {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}
impl BitAndAssign for TraceCategory {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}
impl Not for TraceCategory {
    type Output = Self;
    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

pub const TXN_LIFECYCLE: TraceCategory = TraceCategory(1 << 0);
/// 两阶段提交（2PC）相关事件类别。
pub const TXN_2PC: TraceCategory = TraceCategory(1 << 1);
pub const TXN_LOCK_RESOLVE: TraceCategory = TraceCategory(1 << 2);
pub const STMT_LIFECYCLE: TraceCategory = TraceCategory(1 << 3);
pub const STMT_PLAN: TraceCategory = TraceCategory(1 << 4);
pub const KV_REQUEST: TraceCategory = TraceCategory(1 << 5);
pub const UNKNOWN_CLIENT: TraceCategory = TraceCategory(1 << 6);
pub const GENERAL: TraceCategory = TraceCategory(1 << 7);
pub const DDL_JOB: TraceCategory = TraceCategory(1 << 8);
pub const DEV_DEBUG: TraceCategory = TraceCategory(1 << 9);
pub const TIKV_REQUEST: TraceCategory = TraceCategory(1 << 10);
pub const TIKV_WRITE_DETAILS: TraceCategory = TraceCategory(1 << 11);
pub const TIKV_READ_DETAILS: TraceCategory = TraceCategory(1 << 12);
/// Region 缓存相关事件类别（Region 为 TiKV 键空间分片单位）。
pub const REGION_CACHE: TraceCategory = TraceCategory(1 << 13);
/// 全部已定义类别的位或（低 14 位）。
pub const ALL_CATEGORIES: TraceCategory = TraceCategory((1 << 14) - 1);

/// Go 风格类别别名。
pub const TxnLifecycle: TraceCategory = TXN_LIFECYCLE;
pub const Txn2PC: TraceCategory = TXN_2PC;
pub const TxnLockResolve: TraceCategory = TXN_LOCK_RESOLVE;
pub const StmtLifecycle: TraceCategory = STMT_LIFECYCLE;
pub const StmtPlan: TraceCategory = STMT_PLAN;
pub const KvRequest: TraceCategory = KV_REQUEST;
pub const General: TraceCategory = GENERAL;
pub const UnknownClient: TraceCategory = UNKNOWN_CLIENT;
pub const AllCategories: TraceCategory = ALL_CATEGORIES;

/// 将类别名字符串解析为 TraceCategory；未知名返回 0。
pub fn parse_trace_category(value: &str) -> TraceCategory {
    match value {
        "txn_lifecycle" => TXN_LIFECYCLE,
        "txn_2pc" => TXN_2PC,
        "txn_lock_resolve" => TXN_LOCK_RESOLVE,
        "stmt_lifecycle" => STMT_LIFECYCLE,
        "stmt_plan" => STMT_PLAN,
        "kv_request" => KV_REQUEST,
        "general" => GENERAL,
        "unknown_client" => UNKNOWN_CLIENT,
        "region_cache" => REGION_CACHE,
        "ddl_job" => DDL_JOB,
        "tikv_request" => TIKV_REQUEST,
        "tikv_write_details" => TIKV_WRITE_DETAILS,
        "tikv_read_details" => TIKV_READ_DETAILS,
        "dev_debug" => DEV_DEBUG,
        _ => TraceCategory(0),
    }
}

/// 事件附加字段：字符串键 + JSON 值。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub value: Value,
}

impl Field {
    pub fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: Value::String(value.into()),
        }
    }
    pub fn u32(key: impl Into<String>, value: u32) -> Self {
        Self {
            key: key.into(),
            value: Value::from(value),
        }
    }
    pub fn u64(key: impl Into<String>, value: u64) -> Self {
        Self {
            key: key.into(),
            value: Value::from(value),
        }
    }
    pub fn i64(key: impl Into<String>, value: i64) -> Self {
        Self {
            key: key.into(),
            value: Value::from(value),
        }
    }
    pub fn boolean(key: impl Into<String>, value: bool) -> Self {
        Self {
            key: key.into(),
            value: Value::from(value),
        }
    }
}

/// Chrome Trace Event 风格的相位；当前仅 Instant（瞬时点事件）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    #[serde(rename = "i")]
    Instant,
}

/// 一条结构化追踪事件。
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub category: TraceCategory,
    pub name: String,
    pub phase: Phase,
    pub timestamp_micros: i64,
    pub trace_id: Vec<u8>,
    pub fields: Vec<Field>,
}

/// 追踪上下文：携带 trace_id 与可选的会话 Trace sink。
#[derive(Clone, Default)]
pub struct Context {
    trace_id: Vec<u8>,
    sink: Option<Arc<Trace>>,
}

impl Context {
    pub fn with_trace_id(mut self, trace_id: Vec<u8>) -> Self {
        self.trace_id = trace_id;
        self
    }
    pub fn trace_id(&self) -> &[u8] {
        &self.trace_id
    }
    pub fn with_sink(mut self, sink: Arc<Trace>) -> Self {
        self.sink = Some(sink);
        self
    }
    pub fn sink(&self) -> Option<Arc<Trace>> {
        self.sink.clone()
    }
}

/// 事件接收端：将 Event 写入日志、环形缓冲或会话 Trace。
pub trait Sink: Send + Sync {
    fn record(&self, ctx: &Context, event: Event);
}

static RECORDER_ENABLED: AtomicBool = AtomicBool::new(true);
static LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);
static LAST_DUMP_TIME: AtomicI64 = AtomicI64::new(0);
static EVENT_SINK: OnceLock<RwLock<Arc<dyn Sink>>> = OnceLock::new();
static FLIGHT_RECORDER: OnceLock<Arc<RingBufferSink>> = OnceLock::new();

/// 默认环形飞行记录器容量。
pub const DEFAULT_FLIGHT_RECORDER_CAPACITY: usize = 1024;
pub const DefaultFlightRecorderCapacity: usize = DEFAULT_FLIGHT_RECORDER_CAPACITY;
/// dump 到日志的冷却时间，避免短时间重复刷屏。
pub const FLIGHT_RECORDER_COOLING_OFF_PERIOD: Duration = Duration::from_secs(10);

fn event_sink() -> &'static RwLock<Arc<dyn Sink>> {
    EVENT_SINK.get_or_init(|| RwLock::new(Arc::new(LogSink)))
}

/// 规范化模式字符串（大小写不敏感）；非法值返回错误。
pub fn normalize_mode(mode: &str) -> Result<&'static str, String> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" => Ok(MODE_OFF),
        "base" => Ok(MODE_BASE),
        "full" => Ok(MODE_FULL),
        _ => Err(format!(
            "unsupported trace event mode {:?}, valid modes: off, base, full",
            mode
        )),
    }
}

/// 设置全局 recorder/logging 开关并返回规范化模式名。
pub fn set_mode(mode: &str) -> Result<&'static str, String> {
    let normalized = normalize_mode(mode)?;
    match normalized {
        MODE_OFF => {
            RECORDER_ENABLED.store(false, Ordering::SeqCst);
            LOGGING_ENABLED.store(false, Ordering::SeqCst);
        }
        MODE_BASE => {
            RECORDER_ENABLED.store(true, Ordering::SeqCst);
            LOGGING_ENABLED.store(false, Ordering::SeqCst);
        }
        MODE_FULL => {
            RECORDER_ENABLED.store(true, Ordering::SeqCst);
            LOGGING_ENABLED.store(true, Ordering::SeqCst);
        }
        _ => unreachable!(),
    }
    Ok(normalized)
}

/// 由 RECORDER_ENABLED / LOGGING_ENABLED 推导当前模式。
pub fn current_mode() -> &'static str {
    match (
        RECORDER_ENABLED.load(Ordering::SeqCst),
        LOGGING_ENABLED.load(Ordering::SeqCst),
    ) {
        (false, false) => MODE_OFF,
        (true, false) => MODE_BASE,
        _ => MODE_FULL,
    }
}

/// 类别是否在当前全局飞行记录器的启用集合中。
pub fn is_enabled(category: TraceCategory) -> bool {
    get_flight_recorder()
        .map(|recorder| recorder.enabled_categories().contains(category))
        .unwrap_or(false)
}

/// 当前启用类别位图；无记录器时返回默认 0。
pub fn get_enabled_categories() -> TraceCategory {
    get_flight_recorder()
        .map(|recorder| recorder.enabled_categories())
        .unwrap_or_default()
}

/// 替换全局事件 sink；`None` 则回退到 LogSink。
pub fn set_sink(sink: Option<Arc<dyn Sink>>) {
    *event_sink().write().expect("event sink lock poisoned") =
        sink.unwrap_or_else(|| Arc::new(LogSink));
}

/// 克隆当前全局事件 sink。
pub fn current_sink() -> Arc<dyn Sink> {
    event_sink()
        .read()
        .expect("event sink lock poisoned")
        .clone()
}

/// 进程级环形飞行记录器单例。
pub fn flight_recorder() -> &'static Arc<RingBufferSink> {
    FLIGHT_RECORDER.get_or_init(|| Arc::new(RingBufferSink::new(DEFAULT_FLIGHT_RECORDER_CAPACITY)))
}

/// 记录一条 Instant 事件：类别未启用则直接返回；否则按模式写入各 sink。
pub fn trace_event(ctx: &Context, category: TraceCategory, name: &str, fields: Vec<Field>) {
    if !is_enabled(category) {
        return;
    }
    let event = Event {
        category,
        name: name.to_owned(),
        phase: Phase::Instant,
        timestamp_micros: now_micros(),
        trace_id: trace_id_from_context(ctx),
        fields,
    };
    if RECORDER_ENABLED.load(Ordering::SeqCst) {
        flight_recorder().record(ctx, event.clone());
        if let Some(sink) = ctx.sink() {
            sink.record(ctx, event.clone());
        }
    }
    current_sink().record(ctx, event);
}

/// 从 Context 取出 trace_id 字节副本。
pub fn trace_id_from_context(ctx: &Context) -> Vec<u8> {
    ctx.trace_id().to_vec()
}
/// 复制 Context 并替换 trace_id。
pub fn context_with_trace_id(ctx: &Context, trace_id: Vec<u8>) -> Context {
    let mut result = ctx.clone();
    result.trace_id = trace_id;
    result
}

/// 由事务 start_ts、语句序号与 Trace.random 生成 20 字节 trace_id。
pub fn generate_trace_id(ctx: &Context, start_ts: u64, stmt_count: u64) -> Vec<u8> {
    let mut result = vec![0; 20];
    result[0..8].copy_from_slice(&start_ts.to_be_bytes());
    result[8..16].copy_from_slice(&stmt_count.to_be_bytes());
    let mut suffix = ctx.sink().map(|trace| trace.random()).unwrap_or(0);
    if suffix == 0 {
        suffix = random();
    }
    result[16..20].copy_from_slice(&suffix.to_be_bytes());
    result
}

/// 日志 sink：仅在 LOGGING_ENABLED 时调用 log_event。
pub struct LogSink;
impl Sink for LogSink {
    fn record(&self, ctx: &Context, event: Event) {
        if !LOGGING_ENABLED.load(Ordering::SeqCst) {
            return;
        }
        log_event(ctx, event);
    }
}

/// 将事件格式化为 `[trace-event]` 结构化日志行。
pub(crate) fn log_event(_ctx: &Context, event: Event) {
    let mut fields = Map::new();
    for field in event.fields {
        fields.insert(field.key, field.value);
    }
    fields.insert("category".into(), Value::String(event.category.name()));
    fields.insert("event_ts".into(), Value::from(event.timestamp_micros));
    if !event.trace_id.is_empty() {
        fields.insert(
            "trace_id".into(),
            Value::String(hex_encode(&event.trace_id)),
        );
    }
    log::info!(target: "tidb::traceevent", "[trace-event] {} {}", event.name, Value::Object(fields));
}

/// 扇出到多个下游 Sink（跳过 None）。
pub struct MultiSink {
    sinks: Vec<Arc<dyn Sink>>,
}
impl MultiSink {
    pub fn new(sinks: Vec<Option<Arc<dyn Sink>>>) -> Self {
        Self {
            sinks: sinks.into_iter().flatten().collect(),
        }
    }
}
impl Sink for MultiSink {
    fn record(&self, ctx: &Context, event: Event) {
        for sink in &self.sinks {
            sink.record(ctx, event.clone());
        }
    }
}

struct RingState {
    buf: Vec<Event>,
    next: usize,
}
/// 定容环形缓冲 Sink：写满后覆盖最旧事件；snapshot 按时间序展开。
pub struct RingBufferSink {
    state: Mutex<RingState>,
    capacity: usize,
}
impl RingBufferSink {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            state: Mutex::new(RingState {
                buf: Vec::with_capacity(capacity),
                next: 0,
            }),
            capacity,
        }
    }
    /// 清空缓冲并重置写指针。
    pub fn discard_or_flush(&self) {
        let mut state = self.state.lock().expect("ring buffer lock poisoned");
        state.buf.clear();
        state.next = 0;
    }
    /// 按从旧到新顺序克隆快照；未写满时即当前顺序。
    pub fn snapshot(&self) -> Vec<Event> {
        let state = self.state.lock().expect("ring buffer lock poisoned");
        if state.buf.len() < self.capacity || state.buf.is_empty() {
            return state.buf.clone();
        }
        state.buf[state.next..]
            .iter()
            .chain(state.buf[..state.next].iter())
            .cloned()
            .collect()
    }
}
impl Sink for RingBufferSink {
    fn record(&self, _ctx: &Context, event: Event) {
        let mut state = self.state.lock().expect("ring buffer lock poisoned");
        if state.buf.len() < self.capacity {
            state.buf.push(event);
            if state.buf.len() == self.capacity {
                state.next = 0;
            }
            return;
        }
        let next = state.next;
        state.buf[next] = event;
        state.next = (next + 1) % self.capacity;
    }
}

/// 将环形缓冲快照 dump 到日志；冷却期内或空缓冲返回 0。
pub fn dump_flight_recorder_to_logger(_reason: &str) -> usize {
    dump_flight_recorder_to_logger_at(now_seconds())
}

/// 使用给定秒时间戳执行 dump，供内部测试确定性验证冷却边界。
pub(crate) fn dump_flight_recorder_to_logger_at(now: i64) -> usize {
    let events = flight_recorder().snapshot();
    if events.is_empty() {
        return 0;
    }
    let last = LAST_DUMP_TIME.load(Ordering::SeqCst);
    if last > 0 && now - last < FLIGHT_RECORDER_COOLING_OFF_PERIOD.as_secs() as i64 {
        return 0;
    }
    LAST_DUMP_TIME.store(now, Ordering::SeqCst);
    for event in events.iter().cloned() {
        log_event(&Context::default(), event);
    }
    events.len()
}

/// 重置进程级 dump 冷却状态，避免测试间泄漏。
#[cfg(test)]
pub(crate) fn reset_last_dump_time_for_test() {
    LAST_DUMP_TIME.store(0, Ordering::SeqCst);
}

/// Chrome Trace Event JSON 渲染用结构（ph/ts/pid/tid/cat/args）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderEvent {
    pub name: String,
    #[serde(rename = "ph")]
    pub phase: Phase,
    #[serde(rename = "ts")]
    pub timestamp_micros: i64,
    #[serde(rename = "pid")]
    pub pid: u32,
    #[serde(rename = "tid")]
    pub tid: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u64,
    #[serde(rename = "cat", default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
}

/// 从 20 字节 trace_id 末 4 字节提取 random 后缀作为 tid。
fn extract_rand_from_trace_id(trace_id: &[u8]) -> u32 {
    if trace_id.len() != 20 {
        return 0;
    }
    trace_id
        .get(16..20)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_ne_bytes)
        .unwrap_or(0)
}

/// 将内部 Event 转为渲染结构；fields 与 hex(trace_id) 放入 args。
pub fn convert_events_for_rendering(events: &[Event]) -> Vec<RenderEvent> {
    let tid = events
        .iter()
        .find_map(|event| {
            let value = extract_rand_from_trace_id(&event.trace_id);
            (value != 0).then_some(value)
        })
        .unwrap_or(0);
    events
        .iter()
        .map(|event| {
            let args = if event.fields.is_empty() {
                None
            } else {
                let mut map = Map::new();
                for field in &event.fields {
                    map.insert(field.key.clone(), field.value.clone());
                }
                if !event.trace_id.is_empty() {
                    map.insert(
                        "trace_id".into(),
                        Value::String(hex_encode(&event.trace_id)),
                    );
                }
                Some(Value::Object(map))
            };
            RenderEvent {
                name: event.name.clone(),
                phase: event.phase,
                timestamp_micros: event.timestamp_micros,
                pid: 0,
                tid,
                id: 0,
                category: event.category.name(),
                args,
            }
        })
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0xf) as usize] as char);
    }
    result
}
fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}

pub use context_with_trace_id as ContextWithTraceID;
pub use convert_events_for_rendering as ConvertEventsForRendering;
pub use current_mode as CurrentMode;
pub use current_sink as CurrentSink;
pub use dump_flight_recorder_to_logger as DumpFlightRecorderToLogger;
pub use flight_recorder as FlightRecorder;
pub use generate_trace_id as GenerateTraceID;
pub use get_enabled_categories as GetEnabledCategories;
pub use is_enabled as IsEnabled;
pub use normalize_mode as NormalizeMode;
pub use set_mode as SetMode;
pub use set_sink as SetSink;
pub use trace_event as TraceEvent;
pub use trace_id_from_context as TraceIDFromContext;

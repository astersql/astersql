// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Local stand-ins for PD / oracle / GC safepoint / context / logging
//! (darwin arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 本文件为 GC safepoint 工具测试提供 PD/oracle/Context/日志本地桩。
//! 服务级 safepoint 携带 service_id 与 ttl，供 BR 心跳续约场景断言。
//! GetTS 在未 set_get_ts 时使用 NEXT_PHYSICAL 生成近似真实 TSO。
//! Update* 成功时返回传入的 safe_point，便于链式断言。
//! SecurityOption 不读盘，只保存路径字符串。
//! reset_pd 供用例隔离，避免全局注册表串扰。
//! CancelFunc 与 WithTimeout 配对，覆盖超时取消路径。
//! parse_go_duration 支持 Go time.Duration 的符号、组合与全部单位。
//! 不拨真实 PD：按地址注册 SharedPd，可注入 GetTS/Update* 失败并记录调用。
//! Context 支持取消与超时；CancelFunc Drop 即取消，模拟 defer cancel。
//! oracle 模块实现 ComposeTS/ExtractPhysical 等与 client-go 对齐的子集。
//! parse_go_duration 解析 Go 风格时长，供 CLI TTL 参数单测。

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 本测试包统一 Result。
pub type Result<T> = std::result::Result<T, Error>;

/// Go `time.Duration` 的纳秒表示；与 Go 一样为有符号 64 位整数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GoDuration(i64);

impl GoDuration {
    pub const ZERO: Self = Self(0);

    pub const fn from_secs(seconds: i64) -> Self {
        Self(seconds * 1_000_000_000)
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 轻量错误，仅消息。
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

// 可取消 + 可选 deadline；Done 在超时到达时懒置位 cancelled。
// --- context (cancellation + timeout) ---

#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl Context {
    /// 无超时背景上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 派生带超时上下文，并返回 CancelFunc。
    pub fn WithTimeout(parent: &Self, timeout: Duration) -> (Self, CancelFunc) {
        let ctx = Self {
            cancelled: Arc::new(AtomicBool::new(parent.cancelled.load(Ordering::SeqCst))),
            deadline: Arc::new(Mutex::new(Some(Instant::now() + timeout))),
        };
        let cancel = CancelFunc {
            cancelled: ctx.cancelled.clone(),
        };
        (ctx, cancel)
    }

    /// 已取消或已到 deadline 则 true。
    pub fn Done(&self) -> bool {
        if self.cancelled.load(Ordering::SeqCst) {
            return true;
        }
        if let Some(dl) = *self.deadline.lock().unwrap() {
            if Instant::now() >= dl {
                self.cancelled.store(true, Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    /// Done 时返回 context canceled 错误。
    pub fn Err(&self) -> Option<Error> {
        if self.Done() {
            Some(Error::new("context canceled"))
        } else {
            None
        }
    }

    /// 测试用上下文身份；同一派生 Context 的 clone 保持相同身份。
    pub fn id(&self) -> usize {
        Arc::as_ptr(&self.cancelled) as usize
    }
}

#[derive(Clone)]
/// 显式/Drop 均可触发取消。
pub struct CancelFunc {
    cancelled: Arc<AtomicBool>,
}

impl CancelFunc {
    /// 消费 self 并置位取消标志。
    pub fn cancel(self) {
        // Drop 时自动取消，避免泄漏后台等待。
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Drop for CancelFunc {
    fn drop(&mut self) {
        // Drop 时自动取消，避免泄漏后台等待。
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

// TSO 物理/逻辑位运算与时间换算，无网络。
// --- oracle (tikv client-go oracle subset) ---

pub mod oracle {
    use super::*;

    /// 物理时间左移位数，与 TiDB/TiKV 约定一致。
    pub const PHYSICAL_SHIFT_BITS: u64 = 18;

    /// Go `oracle.ComposeTS(physical, logical)`.
    /// 拼接物理+逻辑为 64 位 TS。
    pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
        ((physical as u64) << PHYSICAL_SHIFT_BITS) + (logical as u64)
    }

    /// Go `oracle.ExtractPhysical(ts)`.
    /// 提取物理毫秒部分。
    pub fn ExtractPhysical(ts: u64) -> i64 {
        (ts >> PHYSICAL_SHIFT_BITS) as i64
    }

    /// Go `oracle.GetTimeFromTS(ts)` — physical millis since Unix epoch.
    /// TS → SystemTime（物理毫秒自 epoch）。
    pub fn GetTimeFromTS(ts: u64) -> SystemTime {
        let ms = ExtractPhysical(ts);
        if ms >= 0 {
            UNIX_EPOCH + Duration::from_millis(ms as u64)
        } else {
            UNIX_EPOCH - Duration::from_millis((-ms) as u64)
        }
    }

    /// Go `oracle.GetPhysical(t)` — Unix millis.
    /// SystemTime → Unix 毫秒（可负）。
    pub fn GetPhysical(t: SystemTime) -> i64 {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            Err(e) => -(e.duration().as_millis() as i64),
        }
    }

    /// Subtract a duration from a SystemTime (Go `t.Add(-d)`).
    /// 时间减去时长；下溢夹到 UNIX_EPOCH。
    pub fn SubDuration(t: SystemTime, d: Duration) -> SystemTime {
        t.checked_sub(d).unwrap_or(UNIX_EPOCH)
    }
}

// 组件名常量，供 NewClientWithContext 记录。
// --- caller ---

pub mod caller {
    /// Go `caller.TestComponent`.
    /// 测试用 caller 组件名。
    pub const TestComponent: &str = "test";
}

// stderr 日志；panic 变体用于致命断言。
// --- logging ---

/// 信息日志。
pub fn log_info(msg: &str, fields: &[(&str, String)]) {
    let mut parts = Vec::new();
    for (k, v) in fields {
        parts.push(format!("{k}={v}"));
    }
    if parts.is_empty() {
        eprintln!("[INFO] {msg}");
    } else {
        eprintln!("[INFO] {msg} {}", parts.join(" "));
    }
}

/// 致命日志后 panic。
pub fn log_panic(msg: &str, fields: &[(&str, String)]) -> ! {
    let mut parts = Vec::new();
    for (k, v) in fields {
        parts.push(format!("{k}={v}"));
    }
    if parts.is_empty() {
        panic!("[PANIC] {msg}");
    } else {
        panic!("[PANIC] {msg} {}", parts.join(" "));
    }
}

// 内存 PD：记录 GC 更新并支持错误注入。
// --- PD client / GC safepoint boundary ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// PD TLS 路径选项占位。
pub struct SecurityOption {
    pub CAPath: String,
    pub CertPath: String,
    pub KeyPath: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 记录的 GC 更新种类：全局或服务级 safepoint。
pub enum GcUpdateKind {
    UpdateGCSafePoint {
        safe_point: u64,
    },
    UpdateServiceGCSafePoint {
        service_id: String,
        ttl: i64,
        safe_point: u64,
    },
}

#[derive(Clone, Default)]
/// 每地址共享状态：TS、注入错误、调用记录。
struct SharedPd {
    /// Fixed (physical, logical) returned by GetTS when set.
    ts: Arc<Mutex<Option<(i64, i64)>>>,
    /// Injected NewClient error.
    new_err: Arc<Mutex<Option<Error>>>,
    /// Injected GetTS error.
    get_ts_err: Arc<Mutex<Option<Error>>>,
    /// Injected UpdateGCSafePoint error.
    update_gc_err: Arc<Mutex<Option<Error>>>,
    /// Injected UpdateServiceGCSafePoint error.
    update_svc_err: Arc<Mutex<Option<Error>>>,
    /// Recorded GC updates (call order).
    updates: Arc<Mutex<Vec<GcUpdateKind>>>,
    /// Last SecurityOption seen at dial.
    security: Arc<Mutex<SecurityOption>>,
    /// Last PD addrs.
    addrs: Arc<Mutex<Vec<String>>>,
    /// Last caller component.
    component: Arc<Mutex<String>>,
    /// 按调用顺序记录 NewClient/GetTS/Update 使用的 Context 身份。
    context_ids: Arc<Mutex<Vec<usize>>>,
    /// 保留同一批 Context 句柄，供用例观测返回后的 cancel 状态。
    contexts: Arc<Mutex<Vec<Context>>>,
    closed: Arc<AtomicBool>,
}

/// 地址→SharedPd 全局表。
fn registry() -> &'static Mutex<std::collections::BTreeMap<String, SharedPd>> {
    static REG: OnceLock<Mutex<std::collections::BTreeMap<String, SharedPd>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()))
}

/// 取或创建某地址的 SharedPd 克隆句柄。
fn store_for(addr: &str) -> SharedPd {
    let mut reg = registry().lock().unwrap();
    reg.entry(addr.to_string()).or_default().clone()
}

/// Test helper: install/clear dialer failure for a PD address.
/// 注入拨号失败。
pub fn set_new_client_error(addr: &str, err: Option<Error>) {
    *store_for(addr).new_err.lock().unwrap() = err;
}

/// Test helper: install GetTS failure.
/// 注入 GetTS 失败。
pub fn set_get_ts_error(addr: &str, err: Option<Error>) {
    *store_for(addr).get_ts_err.lock().unwrap() = err;
}

/// Test helper: fix GetTS physical/logical.
/// 固定 GetTS 返回的物理/逻辑。
pub fn set_get_ts(addr: &str, physical: i64, logical: i64) {
    *store_for(addr).ts.lock().unwrap() = Some((physical, logical));
}

/// Test helper: install UpdateGCSafePoint failure.
/// 注入 UpdateGCSafePoint 失败。
pub fn set_update_gc_error(addr: &str, err: Option<Error>) {
    *store_for(addr).update_gc_err.lock().unwrap() = err;
}

/// Test helper: install UpdateServiceGCSafePoint failure.
/// 注入 UpdateServiceGCSafePoint 失败。
pub fn set_update_service_error(addr: &str, err: Option<Error>) {
    *store_for(addr).update_svc_err.lock().unwrap() = err;
}

/// Test helper: recorded GC update calls.
/// 读取按调用序记录的 GC 更新。
pub fn recorded_updates(addr: &str) -> Vec<GcUpdateKind> {
    store_for(addr).updates.lock().unwrap().clone()
}

/// Test helper: clear recorded updates.
/// 清空更新记录。
pub fn clear_updates(addr: &str) {
    store_for(addr).updates.lock().unwrap().clear();
}

/// Test helper: last SecurityOption at dial.
/// 上次拨号见到的 SecurityOption。
pub fn last_security(addr: &str) -> SecurityOption {
    store_for(addr).security.lock().unwrap().clone()
}

/// Test helper: last dialed addrs / component.
/// 上次拨号的地址列表与组件名。
pub fn last_dial(addr: &str) -> (Vec<String>, String) {
    let s = store_for(addr);
    (
        s.addrs.lock().unwrap().clone(),
        s.component.lock().unwrap().clone(),
    )
}

/// Test helper: contexts observed by dial, GetTS and update, in call order.
pub fn recorded_context_ids(addr: &str) -> Vec<usize> {
    store_for(addr).context_ids.lock().unwrap().clone()
}

/// Test helper: cancellation state of contexts observed by PD calls, in call order.
pub fn recorded_context_done(addr: &str) -> Vec<bool> {
    store_for(addr)
        .contexts
        .lock()
        .unwrap()
        .iter()
        .map(Context::Done)
        .collect()
}

/// Test helper: reset all injectable state for an address.
/// 重置某地址全部可注入状态。
pub fn reset_pd(addr: &str) {
    let s = store_for(addr);
    *s.ts.lock().unwrap() = None;
    *s.new_err.lock().unwrap() = None;
    *s.get_ts_err.lock().unwrap() = None;
    *s.update_gc_err.lock().unwrap() = None;
    *s.update_svc_err.lock().unwrap() = None;
    s.updates.lock().unwrap().clear();
    *s.security.lock().unwrap() = SecurityOption::default();
    s.addrs.lock().unwrap().clear();
    s.component.lock().unwrap().clear();
    s.context_ids.lock().unwrap().clear();
    s.contexts.lock().unwrap().clear();
    s.closed.store(false, Ordering::SeqCst);
}

/// 未固定 TS 时递增的默认物理时间。
static NEXT_PHYSICAL: AtomicU64 = AtomicU64::new(1_700_000_000_000);

/// PD 客户端桩：绑定地址键与 SharedPd。
pub struct PdClient {
    addr_key: String,
    store: SharedPd,
}

impl Clone for PdClient {
    fn clone(&self) -> Self {
        Self {
            addr_key: self.addr_key.clone(),
            store: self.store.clone(),
        }
    }
}

/// Go `pd.NewClientWithContext(ctx, component, addrs, security)`.
/// 对齐 Go NewClientWithContext；可因 new_err 失败。
pub fn NewClientWithContext(
    ctx: &Context,
    component: &str,
    addrs: Vec<String>,
    security: SecurityOption,
) -> Result<PdClient> {
    let key = addrs.first().cloned().unwrap_or_default();
    let store = store_for(&key);
    *store.security.lock().unwrap() = security;
    *store.addrs.lock().unwrap() = addrs;
    *store.component.lock().unwrap() = component.to_string();
    store.context_ids.lock().unwrap().push(ctx.id());
    store.contexts.lock().unwrap().push(ctx.clone());
    if let Some(err) = store.new_err.lock().unwrap().clone() {
        return Err(err);
    }
    Ok(PdClient {
        addr_key: key,
        store,
    })
}

impl PdClient {
    /// Go `pdclient.GetTS(ctx)` → (physical, logical).
    /// 返回 (physical, logical)；尊重 ctx 取消与注入错误。
    pub fn GetTS(&self, ctx: &Context) -> Result<(i64, i64)> {
        self.store.context_ids.lock().unwrap().push(ctx.id());
        self.store.contexts.lock().unwrap().push(ctx.clone());
        // 取消优先于注入错误与成功路径。
        if ctx.Done() {
            return Err(Error::new("context canceled"));
        }
        if let Some(err) = self.store.get_ts_err.lock().unwrap().clone() {
            return Err(err);
        }
        // 测试固定 TS 优先于自动递增。
        if let Some(ts) = *self.store.ts.lock().unwrap() {
            return Ok(ts);
        }
        // 默认物理时间单调递增，逻辑位为 0。
        let physical = NEXT_PHYSICAL.fetch_add(1, Ordering::SeqCst) as i64;
        Ok((physical, 0))
    }

    /// Go `pdclient.UpdateGCSafePoint(ctx, safePoint)`.
    /// 记录全局 GC safepoint 更新并回显。
    pub fn UpdateGCSafePoint(&self, ctx: &Context, safe_point: u64) -> Result<u64> {
        self.store.context_ids.lock().unwrap().push(ctx.id());
        self.store.contexts.lock().unwrap().push(ctx.clone());
        // 取消优先于注入错误与成功路径。
        if ctx.Done() {
            return Err(Error::new("context canceled"));
        }
        if let Some(err) = self.store.update_gc_err.lock().unwrap().clone() {
            return Err(err);
        }
        self.store
            .updates
            .lock()
            .unwrap()
            .push(GcUpdateKind::UpdateGCSafePoint { safe_point });
        Ok(safe_point)
    }

    /// Go `pdclient.UpdateServiceGCSafePoint(ctx, serviceID, ttl, safePoint)`.
    /// 记录服务级 safepoint（含 TTL）并回显。
    pub fn UpdateServiceGCSafePoint(
        &self,
        ctx: &Context,
        service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64> {
        self.store.context_ids.lock().unwrap().push(ctx.id());
        self.store.contexts.lock().unwrap().push(ctx.clone());
        // 取消优先于注入错误与成功路径。
        if ctx.Done() {
            return Err(Error::new("context canceled"));
        }
        if let Some(err) = self.store.update_svc_err.lock().unwrap().clone() {
            return Err(err);
        }
        self.store
            .updates
            .lock()
            .unwrap()
            .push(GcUpdateKind::UpdateServiceGCSafePoint {
                service_id: service_id.to_string(),
                ttl,
                safe_point,
            });
        Ok(safe_point)
    }

    /// 标记关闭。
    pub fn Close(&self) {
        self.store.closed.store(true, Ordering::SeqCst);
    }

    /// 是否已 Close。
    pub fn is_closed(&self) -> bool {
        self.store.closed.load(Ordering::SeqCst)
    }

    /// 注册表键（通常为首地址）。
    pub fn addr_key(&self) -> &str {
        &self.addr_key
    }
}

/// Parse Go `time.ParseDuration` strings into signed nanoseconds.
/// 支持正负号、组合、小数，以及 ns/us/µs/μs/ms/s/m/h。
pub fn parse_go_duration(s: &str) -> Result<GoDuration> {
    if s.is_empty() {
        return Err(Error::new("empty duration"));
    }

    let (negative, body) = match s.as_bytes()[0] {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if body == "0" {
        return Ok(GoDuration::ZERO);
    }
    if body.is_empty() {
        return Err(Error::new(format!("invalid duration: {s}")));
    }

    let mut rest = body;
    let mut total = 0i128;
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        let mut integer_end = 0usize;
        while integer_end < bytes.len() && bytes[integer_end].is_ascii_digit() {
            integer_end += 1;
        }
        let integer = if integer_end == 0 {
            0i128
        } else {
            rest[..integer_end]
                .parse::<i128>()
                .map_err(|_| Error::new(format!("invalid duration: {s}")))?
        };

        let mut number_end = integer_end;
        let mut fraction = 0i128;
        let mut scale = 1i128;
        if number_end < bytes.len() && bytes[number_end] == b'.' {
            number_end += 1;
            let fraction_start = number_end;
            while number_end < bytes.len() && bytes[number_end].is_ascii_digit() {
                fraction = fraction
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((bytes[number_end] - b'0') as i128))
                    .ok_or_else(|| Error::new(format!("invalid duration: {s}")))?;
                scale = scale
                    .checked_mul(10)
                    .ok_or_else(|| Error::new(format!("invalid duration: {s}")))?;
                number_end += 1;
            }
            if integer_end == 0 && fraction_start == number_end {
                return Err(Error::new(format!("invalid duration: {s}")));
            }
        } else if integer_end == 0 {
            return Err(Error::new(format!("invalid duration: {s}")));
        }

        let unit_rest = &rest[number_end..];
        let (unit, unit_nanos) = [
            ("ns", 1i128),
            ("us", 1_000),
            ("µs", 1_000),
            ("μs", 1_000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find(|(unit, _)| unit_rest.starts_with(unit))
        .ok_or_else(|| Error::new(format!("missing or invalid duration unit: {s}")))?;

        let whole = integer
            .checked_mul(unit_nanos)
            .ok_or_else(|| Error::new(format!("duration out of range: {s}")))?;
        let fractional = fraction
            .checked_mul(unit_nanos)
            .ok_or_else(|| Error::new(format!("duration out of range: {s}")))?
            / scale;
        total = total
            .checked_add(whole)
            .and_then(|v| v.checked_add(fractional))
            .ok_or_else(|| Error::new(format!("duration out of range: {s}")))?;
        rest = &unit_rest[unit.len()..];
    }

    let signed = if negative { -total } else { total };
    let nanos =
        i64::try_from(signed).map_err(|_| Error::new(format!("duration out of range: {s}")))?;
    Ok(GoDuration(nanos))
}

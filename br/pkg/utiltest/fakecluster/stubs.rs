// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! Local stand-ins for kvproto / grpc / oracle / codec / txnlock boundaries
//! (darwin arm64-safe; no kv/domain/kvproto/grpcio).
//! 假集群桩类型：kvproto/gRPC/oracle/codec/txnlock 边界替身。
//! 供 core.rs 在无真实 protobuf/gRPC 依赖下编译运行。
//! 含 KeyRange、日志备份 RPC 消息、StatusError、Context 与 oracle/codec 工具。
//! 非生产代码；锁与 flush 消息仅为内存结构，无网络语义。
//! Error 可从 StatusError 转换，便于 Store RPC 返回。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// --- kv.KeyRange ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 键范围 [StartKey, EndKey)，空 EndKey 表示 +∞。
pub struct KeyRange {
    /// 范围起点（含）。
    pub StartKey: Vec<u8>,
    /// 范围终点（不含）；空=无上界。
    pub EndKey: Vec<u8>,
}

// --- errorpb ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// errorpb 风格错误，仅 Message 字段。
pub struct ErrorPb {
    /// 错误消息文本。
    pub Message: String,
}

// --- logbackuppb ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Region 身份：Id + EpochVersion。
pub struct RegionIdentity {
    /// Region ID。
    pub Id: u64,
    /// Epoch 版本，用于匹配校验。
    pub EpochVersion: u64,
}

#[derive(Clone, Debug, Default)]
/// 单 region 的 checkpoint 查询结果。
pub struct RegionCheckpoint {
    /// 查询失败时的 region 错误。
    pub Err: Option<ErrorPb>,
    /// 对应 region 身份。
    pub Region: Option<RegionIdentity>,
    /// 成功时的 checkpoint TS。
    pub Checkpoint: u64,
}

#[derive(Clone, Debug, Default)]
/// 批量查询 region 最近 flush TS 的请求。
pub struct GetLastFlushTSOfRegionRequest {
    /// 待查询 region 列表。
    pub Regions: Vec<RegionIdentity>,
}

#[derive(Clone, Debug, Default)]
/// 批量查询响应。
pub struct GetLastFlushTSOfRegionResponse {
    /// 与请求顺序对应的结果。
    pub Checkpoints: Vec<RegionCheckpoint>,
}

#[derive(Clone, Debug, Default)]
/// 订阅流上的一次 flush 事件。
pub struct FlushEvent {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    /// 事件携带的 checkpoint。
    pub Checkpoint: u64,
}

#[derive(Clone, Debug, Default)]
/// 订阅响应：一批 FlushEvent。
pub struct SubscribeFlushEventResponse {
    /// 本批次事件。
    pub Events: Vec<FlushEvent>,
}

#[derive(Clone, Debug, Default)]
/// 订阅请求占位（无字段）。
pub struct SubscribeFlushEventRequest {}

#[derive(Clone, Debug, Default)]
/// 立即 flush 请求占位。
pub struct FlushNowRequest {}

#[derive(Clone, Debug, Default)]
/// 单任务 flush 结果。
pub struct FlushResult {
    /// 任务名。
    pub TaskName: String,
    /// 是否成功。
    pub Success: bool,
}

#[derive(Clone, Debug, Default)]
/// FlushNow 响应。
pub struct FlushNowResponse {
    /// 各任务结果。
    pub Results: Vec<FlushResult>,
}

// --- txnlock ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 事务锁替身，供 LockRegion 注入。
pub struct Lock {
    /// 锁键。
    pub Key: Vec<u8>,
    /// 主锁键。
    pub Primary: Vec<u8>,
    /// 事务 ID。
    pub TxnID: u64,
    /// 锁 TTL。
    pub TTL: u64,
}

// --- grpc status ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// gRPC 风格状态码子集。
pub enum Code {
    /// 上下文取消。
    Canceled,
    /// 方法未实现/已禁用。
    Unimplemented,
}

#[derive(Clone, Debug)]
/// 带 code 的状态错误。
pub struct StatusError {
    /// 状态码。
    pub code: Code,
    /// 状态消息。
    pub message: String,
}

impl StatusError {
    /// 构造 StatusError。
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for StatusError {}

/// 快捷构造 StatusError。
pub fn status_error(code: Code, message: impl Into<String>) -> StatusError {
    StatusError::new(code, message)
}

// --- context ---

#[derive(Clone)]
/// 可取消 Context，含 wait_cancelled。
pub struct Context {
    inner: Arc<ContextInner>,
}

// Context 共享内部状态。
struct ContextInner {
    cancelled: AtomicBool,
    err: Mutex<Option<String>>,
    pair: (Mutex<()>, Condvar),
}

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// 永不取消上下文。
    pub fn background() -> Self {
        Self {
            inner: Arc::new(ContextInner {
                cancelled: AtomicBool::new(false),
                err: Mutex::new(None),
                pair: (Mutex::new(()), Condvar::new()),
            }),
        }
    }

    /// 可取消对。
    pub fn with_cancel() -> (Self, CancelHandle) {
        let ctx = Self::background();
        let handle = CancelHandle {
            inner: Arc::clone(&ctx.inner),
        };
        (ctx, handle)
    }

    /// 是否已取消。
    pub fn is_done(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 取消错误消息。
    pub fn err_message(&self) -> Option<String> {
        self.inner.err.lock().unwrap().clone()
    }

    /// 阻塞直到取消。
    pub fn wait_cancelled(&self) {
        let (lock, cv) = &self.inner.pair;
        let mut guard = lock.lock().unwrap();
        while !self.inner.cancelled.load(Ordering::SeqCst) {
            guard = cv.wait(guard).unwrap();
        }
    }
}

#[derive(Clone)]
/// 取消句柄。
pub struct CancelHandle {
    inner: Arc<ContextInner>,
}

impl CancelHandle {
    /// 默认消息取消。
    pub fn cancel(&self) {
        self.cancel_with("context canceled");
    }

    /// 带消息取消并唤醒等待者。
    pub fn cancel_with(&self, msg: impl Into<String>) {
        {
            let mut err = self.inner.err.lock().unwrap();
            if err.is_none() {
                *err = Some(msg.into());
            }
        }
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.pair.1.notify_all();
    }
}

// --- oracle (tikv client-go oracle subset) ---

// oracle：TSO 拼装/拆解与时间转换。
pub mod oracle {
    use super::*;

    /// 物理时间左移位数（18）。
    pub const PHYSICAL_SHIFT_BITS: u64 = 18;

    /// 物理+逻辑拼成 TSO。
    pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
        ((physical as u64) << PHYSICAL_SHIFT_BITS).wrapping_add(logical as u64)
    }

    /// 从 TSO 提取物理毫秒。
    pub fn ExtractPhysical(ts: u64) -> i64 {
        (ts >> PHYSICAL_SHIFT_BITS) as i64
    }

    /// TSO → SystemTime。
    pub fn GetTimeFromTS(ts: u64) -> SystemTime {
        let ms = ExtractPhysical(ts);
        let secs = ms / 1000;
        let nsecs = (ms % 1000) * 1_000_000;
        if secs >= 0 {
            UNIX_EPOCH + Duration::new(secs as u64, nsecs as u32)
        } else {
            UNIX_EPOCH - Duration::new((-secs) as u64, 0) + Duration::from_nanos(nsecs as u64)
        }
    }

    /// SystemTime → TSO（逻辑位为 0）。
    pub fn GoTimeToTS(t: SystemTime) -> u64 {
        let ms = match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            Err(e) => -(e.duration().as_millis() as i64),
        };
        (ms as u64) << PHYSICAL_SHIFT_BITS
    }

    pub fn add_duration(t: SystemTime, d: Duration) -> SystemTime {
        t + d
    }
}

// --- codec.EncodeBytes (memcomparable) ---

// codec：EncodeBytes 等 TiKV 键编码替身。
pub mod codec {
    const ENC_GROUP_SIZE: usize = 8;
    const ENC_MARKER: u8 = 0xFF;
    const ENC_PAD: u8 = 0x0;

    /// Matches `pkg/util/codec.EncodeBytes`.
    pub fn EncodeBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
        let d_len = data.len();
        let realloc_size = (d_len / ENC_GROUP_SIZE + 1) * (ENC_GROUP_SIZE + 1);
        b.reserve(realloc_size);

        let mut idx = 0;
        while idx <= d_len {
            let remain = d_len - idx;
            let pad_count;
            if remain >= ENC_GROUP_SIZE {
                b.extend_from_slice(&data[idx..idx + ENC_GROUP_SIZE]);
                pad_count = 0;
            } else {
                pad_count = ENC_GROUP_SIZE - remain;
                b.extend_from_slice(&data[idx..]);
                b.extend(std::iter::repeat(ENC_PAD).take(pad_count));
            }
            b.push(ENC_MARKER - pad_count as u8);
            idx += ENC_GROUP_SIZE;
        }
        b
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
/// core 使用的通用错误（含 msg）。
pub struct Error {
    /// 错误消息。
    pub msg: String,
    pub status: Option<StatusError>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            status: None,
        }
    }

    pub fn from_status(s: StatusError) -> Self {
        Self {
            msg: s.to_string(),
            status: Some(s),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

// StatusError 转为 Error，保留文案。
impl From<StatusError> for Error {
    fn from(s: StatusError) -> Self {
        Self::from_status(s)
    }
}

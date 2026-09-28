// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! 本地桩：PD/TiKV/codec/retry 边界（darwin 安全，无 kvproto/grpcio）。
//! 为 split 包单测与轻量集成提供 Context、重试、metapb 等最小替身。
//! 行为对齐 Go 测试桩语义，但未实现真实 RPC；勿当作生产客户端。
//! Local stand-ins for PD/TiKV/codec/retry boundaries (darwin-safe, no kvproto/grpcio).

use std::cmp::Ordering;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_errors::{New, SharedError};

/// 本包统一 Result，错误为 SharedError。
pub type Result<T> = std::result::Result<T, SharedError>;

/// Mirrors Go failpoint `hint-scan-region-backoff` (microsecond delay).
/// 镜像 Go failpoint `hint-scan-region-backoff`（微秒级延迟开关）。
pub static HINT_SCAN_REGION_BACKOFF: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Default)]
/// 可取消上下文替身：保留父取消传播与 deadline 语义。
pub struct Context {
    cancelled: Arc<Mutex<Option<SharedError>>>,
    parent: Option<Arc<Context>>,
    deadline: Option<Instant>,
}

impl Context {
    /// 永不取消的根上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 派生可超时子上下文；显式 cancel 与 deadline 到期采用不同错误。
    pub fn WithTimeout(parent: &Context, timeout: Duration) -> (Self, impl FnOnce()) {
        let child = Context {
            cancelled: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent.clone())),
            deadline: Some(Instant::now() + timeout),
        };
        let cancel = {
            let c = child.clone();
            move || {
                if c.Err().is_none() {
                    c.cancel(SharedError::new(Canceled));
                }
            }
        };
        (child, cancel)
    }

    /// 派生可取消子上下文。
    pub fn WithCancel(parent: &Context) -> (Self, Box<dyn FnOnce() + Send>) {
        let child = Context {
            cancelled: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent.clone())),
            deadline: None,
        };
        let flag = child.cancelled.clone();
        let cancel = Box::new(move || {
            *flag.lock().unwrap() = Some(SharedError::new(Canceled));
            // 主动取消写入 Canceled。
        });
        (child, cancel)
    }

    /// 写入取消错误，唤醒 Done/Err 观察者。
    pub fn cancel(&self, err: SharedError) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// 若已取消则返回错误副本。
    pub fn Err(&self) -> Option<SharedError> {
        if let Some(err) = self.cancelled.lock().unwrap().clone() {
            return Some(err);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Some(SharedError::new(DeadlineExceeded));
        }
        self.parent.as_ref().and_then(|parent| parent.Err())
    }

    /// 是否已取消。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// 超时错误标记类型。
pub struct DeadlineExceeded;

impl fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context deadline exceeded")
    }
}

impl std::error::Error for DeadlineExceeded {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// 主动取消错误标记类型。
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context canceled")
    }
}

impl std::error::Error for Canceled {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 半开键区间 [StartKey, EndKey)。
pub struct KeyRange {
    /// 区间起点（含）。
    pub StartKey: Vec<u8>,
    /// 区间终点（不含）；空表示正无穷。
    pub EndKey: Vec<u8>,
}

/// metapb Region/Peer 最小结构。
pub mod metapb {
    use super::RegionEpoch;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// Region 内 Peer：Id 与 StoreId。
    pub struct Peer {
        /// 标识 Id。
        pub Id: u64,
        /// 所在 store。
        pub StoreId: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// Region 元数据最小字段集。
    pub struct Region {
        /// 标识 Id。
        pub Id: u64,
        /// 起点键。
        pub StartKey: Vec<u8>,
        /// 终点键。
        pub EndKey: Vec<u8>,
        /// 可选纪元。
        pub RegionEpoch: Option<RegionEpoch>,
        /// Peer 列表。
        pub Peers: Vec<Peer>,
    }

    impl Region {
        /// 返回 StartKey 切片。
        pub fn GetStartKey(&self) -> &[u8] {
            &self.StartKey
        }
        /// 返回 EndKey 切片。
        pub fn GetEndKey(&self) -> &[u8] {
            &self.EndKey
        }
        /// 返回 Region Id。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// Store 及其标签。
    pub struct Store {
        /// 标识 Id。
        pub Id: u64,
        /// 拓扑标签。
        pub Labels: Vec<StoreLabel>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// Store 拓扑标签键值。
    pub struct StoreLabel {
        /// 标签键。
        pub Key: String,
        /// 标签值。
        pub Value: String,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Region 纪元：ConfVer + Version。
pub struct RegionEpoch {
    /// 配置版本。
    pub ConfVer: u64,
    /// 数据版本。
    pub Version: u64,
}

impl RegionEpoch {
    /// 格式化纪元，便于日志。
    pub fn String(&self) -> String {
        format!("{{conf_ver:{}, version:{}}}", self.ConfVer, self.Version)
    }
}

/// pdpb 请求/响应最小结构。
pub mod pdpb {
    #[derive(Clone, Debug, Default)]
    /// PD 响应头，可携 Error。
    pub struct ResponseHeader {
        /// 可选错误。
        pub Error: Option<Error>,
    }

    #[derive(Clone, Debug, Default)]
    /// PD 错误类型与消息。
    pub struct Error {
        /// 错误类型。
        pub Type: ErrorType,
        /// 错误消息。
        pub Message: String,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    /// 与 pdpb.ErrorType 部分枚举对齐的桩。
    pub enum ErrorType {
        #[default]
        OK = 0,
        UNKNOWN = 1,
        REGION_NOT_FOUND = 5,
        DATA_COMPACTED = 7,
    }

    impl ErrorType {
        /// 错误类型名字符串。
        pub fn as_str(self) -> &'static str {
            match self {
                ErrorType::OK => "OK",
                ErrorType::UNKNOWN => "UNKNOWN",
                ErrorType::REGION_NOT_FOUND => "REGION_NOT_FOUND",
                ErrorType::DATA_COMPACTED => "DATA_COMPACTED",
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    /// GetOperator 响应桩。
    pub struct GetOperatorResponse {
        /// 响应头。
        pub Header: Option<ResponseHeader>,
        /// Operator 描述。
        pub Desc: Vec<u8>,
        /// Operator 状态。
        pub Status: OperatorStatus,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    /// Operator 状态枚举桩。
    pub enum OperatorStatus {
        #[default]
        SUCCESS = 0,
        RUNNING = 1,
        CANCEL = 2,
        REPLACE = 3,
        TIMEOUT = 4,
    }

    #[derive(Clone, Debug, Default)]
    /// ScatterRegion 响应桩。
    pub struct ScatterRegionResponse {
        /// 响应头。
        pub Header: Option<ResponseHeader>,
        /// 完成百分比。
        pub FinishedPercentage: u64,
        /// 失败 region id 列表。
        pub FailedRegionsId: Vec<u64>,
    }
}

/// PD HTTP API 替身类型。
pub mod pdhttp {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// Placement Rule 替身。
    pub struct Rule {
        /// 规则组 ID。
        pub GroupID: String,
        /// 规则 ID。
        pub ID: String,
        /// 规则起点（字符串）。
        pub StartKey: String,
        /// 规则终点（字符串）。
        pub EndKey: String,
    }
}

/// TiDB 键编码桩：EncodeBytes/DecodeBytes。
pub mod codec {
    use astersql_br_pkg_restore_utils::stubs::codec as utils_codec;

    pub use utils_codec::{DecodeBytes, EncodeBytes};

    /// 按 is_raw_kv 选择是否做 memcomparable 编码。
    pub fn EncodeBytesExt(b: Vec<u8>, data: &[u8], is_raw_kv: bool) -> Vec<u8> {
        if is_raw_kv {
            let mut out = b;
            out.extend_from_slice(data);
            out
        } else {
            EncodeBytes(b, data)
        }
    }
}

/// 表前缀编码桩。
pub mod tablecodec {
    pub use astersql_br_pkg_restore_utils::stubs::tablecodec::*;

    /// Int handle encoded as big-endian i64 (TiDB row handle).
    /// 整型 handle 编码。
    pub fn IntHandle(h: i64) -> Vec<u8> {
        let u = (h as u64) ^ (1u64 << 63);
        u.to_be_bytes().to_vec()
    }

    /// 表 ID + handle 组成行键。
    pub fn EncodeRowKeyWithHandle(table_id: i64, handle: &[u8]) -> Vec<u8> {
        let mut key = GenTableRecordPrefix(table_id);
        key.extend_from_slice(handle);
        key
    }

    /// Common-handle style: encode int datums as successive EncodeInt chunks.
    /// 多列 common handle 编码。
    pub fn EncodeCommonHandle(datums: &[i64]) -> Vec<u8> {
        let mut out = Vec::new();
        for d in datums {
            out.extend_from_slice(&IntHandle(*d));
        }
        out
    }
}

/// Test helper: enable Go failpoint-equivalent for scan-region backoff.
/// 测试辅助：打开 scan-region backoff failpoint 开关。
pub fn EnableHintScanRegionBackoff(on: bool) {
    HINT_SCAN_REGION_BACKOFF.store(on, AtomicOrdering::SeqCst);
}

/// 脱敏展示桩。
pub mod redact {
    /// 键的脱敏/hex 展示。
    pub fn Key(key: &[u8]) -> String {
        key.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// 日志/展示辅助桩。
pub mod logutil {
    use super::metapb;
    use super::redact;

    /// 区间字符串化。
    pub fn StringifyRange(start: &[u8], end: &[u8]) -> String {
        format!("[{}, {})", redact::Key(start), redact::Key(end))
    }

    /// 区间字符串化（别名路径）。
    pub fn StringifyRangeOf(start: &[u8], end: &[u8]) -> String {
        StringifyRange(start, end)
    }

    /// Region 展示串（桩返回固定占位）。
    pub fn Region(_r: &metapb::Region) -> String {
        "region".into()
    }

    /// 命名键展示（桩）。
    pub fn Key(_name: &str, _key: &[u8]) -> String {
        "key".into()
    }

    /// 错误短文本。
    pub fn ShortError(err: &dyn std::fmt::Display) -> String {
        err.to_string()
    }
}

/// 轻量日志桩：Info/Warn/Debug/Fatal。
pub mod log {
    /// Info 级别日志（空实现）。
    pub fn Info(_msg: &str) {}
    /// Warn 级别日志（空实现）。
    pub fn Warn(_msg: &str) {}
    /// Debug 级别日志（空实现）。
    pub fn Debug(_msg: &str) {}
    /// Error 级别日志（空实现）。
    pub fn Error(_msg: &str) {}
    /// Fatal：打印后 panic，对齐 Go log.Fatal。
    pub fn Fatal(_msg: &str) -> ! {
        panic!("log.Fatal")
    }
}

/// 比较 EndKey；空键视为正无穷。
pub fn CompareEndKey(a: &[u8], b: &[u8]) -> i32 {
    if a.is_empty() {
        return if b.is_empty() { 0 } else { 1 };
    }
    if b.is_empty() {
        return -1;
    }
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// 字节序比较，空键可按“正无穷”处理（与 Go 工具一致）。
pub fn CompareBytesExt(a: &[u8], a_empty_as_inf: bool, b: &[u8], b_empty_as_inf: bool) -> i32 {
    if a.is_empty() && a_empty_as_inf && b.is_empty() && b_empty_as_inf {
        return 0;
    }
    if a.is_empty() && a_empty_as_inf {
        return 1;
    }
    if b.is_empty() && b_empty_as_inf {
        return -1;
    }
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// kv.Key.Next — appends 0x00.
/// 返回字典序下一键（末字节 +1 或追加 0）。
pub fn KeyNext(key: &[u8]) -> Vec<u8> {
    let mut out = key.to_vec();
    out.push(0);
    out
}

/// 重试退避策略接口。
pub trait BackoffStrategy {
    /// 计算下一次退避时长。
    fn NextBackoff(&mut self, err: &SharedError) -> Duration;
    /// 剩余重试次数。
    fn RemainingAttempts(&self) -> i32;
}

#[derive(Clone, Debug)]
/// 重试状态机：剩余次数与退避。
pub struct RetryState {
    max_retry: i32,
    retry_times: i32,
    max_backoff: Duration,
    next_backoff: Duration,
}

/// 构造初始重试状态。
pub fn InitialRetryState(
    max_retry_times: i32,
    initial_backoff: Duration,
    max_backoff: Duration,
) -> RetryState {
    RetryState {
        max_retry: max_retry_times,
        max_backoff,
        next_backoff: initial_backoff,
        retry_times: 0,
    }
}

impl RetryState {
    /// 指数退避并消耗一次尝试。
    pub fn ExponentialBackoff(&mut self) -> Duration {
        self.retry_times += 1;
        let backoff = self.next_backoff;
        self.next_backoff = self.next_backoff.saturating_mul(2);
        if self.next_backoff > self.max_backoff {
            self.next_backoff = self.max_backoff;
        }
        backoff
    }

    /// 将剩余次数清零。
    pub fn GiveUp(&mut self) {
        self.retry_times = self.max_retry;
    }

    /// 减少一次重试计数。
    pub fn ReduceRetry(&mut self) {
        self.retry_times -= 1;
    }

    pub fn RemainingAttempts(&self) -> i32 {
        self.max_retry - self.retry_times
    }
}

impl BackoffStrategy for RetryState {
    fn NextBackoff(&mut self, _err: &SharedError) -> Duration {
        self.ExponentialBackoff()
    }

    fn RemainingAttempts(&self) -> i32 {
        RetryState::RemainingAttempts(self)
    }
}

/// 带重试执行闭包，对齐 Go WithRetry。
pub fn WithRetry<F>(
    ctx: &Context,
    mut retryable: F,
    backoff: &mut dyn BackoffStrategy,
) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    let mut last_err: Option<SharedError> = None;
    // Mirror Go utils.WithRetry: attempt while RemainingAttempts > 0, then one
    // final observation that NextBackoff may zero-out attempts.
    loop {
        if backoff.RemainingAttempts() <= 0 {
            break;
        }
        match retryable() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err.clone());
                if ctx.Done() {
                    return Err(err);
                }
                let delay = backoff.NextBackoff(&err);
                if delay.is_zero() {
                    // Non-retryable / give-up path.
                    if backoff.RemainingAttempts() <= 0 {
                        break;
                    }
                }
                // Keep tests fast: skip real sleep for long delays.
                if delay > Duration::from_millis(1) {
                    std::thread::sleep(Duration::from_micros(1));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| New("retry exhausted")))
}

/// 重试后仍失败则返回最后一次错误。
pub fn WithRetryReturnLastErr<F>(
    ctx: &Context,
    mut retryable: F,
    backoff: &mut dyn BackoffStrategy,
) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    if let Some(err) = ctx.Err() {
        return Err(err);
    }
    let mut last_err: Option<SharedError> = None;
    while backoff.RemainingAttempts() > 0 {
        match retryable() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err.clone());
                let delay = backoff.NextBackoff(&err);
                if ctx.Done() {
                    return Err(err);
                }
                if delay > Duration::from_millis(1) {
                    std::thread::sleep(Duration::from_micros(1));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| New("retry exhausted")))
}

/// Minimal codec PD client handle for GetCodecPDClient.
#[derive(Clone, Debug, Default)]
/// Codec 感知的 PD 客户端桩（仅持 Codec）。
pub struct CodecPDClient;

impl CodecPDClient {
    /// 取出 Codec。
    pub fn GetCodec(&self) -> Codec {
        Codec
    }
}

#[derive(Clone, Debug, Default)]
/// 键范围编解码桩。
pub struct Codec;

impl Codec {
    /// 解码范围（桩：原样返回）。
    pub fn DecodeRange(&self, start: &[u8], end: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        Ok((start.to_vec(), end.to_vec()))
    }

    /// 编码 region 范围（桩：原样返回）。
    pub fn EncodeRegionRange(&self, start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>) {
        (start.to_vec(), end.to_vec())
    }
}

/// PD GetRegion option reserved slot.
#[derive(Clone, Copy, Debug, Default)]
/// GetRegion 选项桩。
pub struct GetRegionOption {
    /// 是否允许 follower 处理。
    pub allow_follower: bool,
}

/// 标记允许 follower 处理（桩：透传）。
pub fn WithAllowFollowerHandle() -> GetRegionOption {
    GetRegionOption {
        allow_follower: true,
    }
}

#[derive(Clone, Copy, Debug, Default)]
/// GetStore 选项桩。
pub struct GetStoreOption;

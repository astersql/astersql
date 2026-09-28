// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Batch Coprocessor 请求发送与重试。
//
// 定义 Region（键空间分片）、RPC 上下文、退避（backoff）与 `RegionBatchRequestSender`：
// 将合并后的 BatchCop 请求发往 TiFlash/TiKV，并在传输失败时决定是否重建任务后重试。

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Batch Cop 发送路径上的错误分类（取消、关停、传输、退避耗尽、无存活 store 等）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BatchError {
    Cancelled,
    ShuttingDown,
    Transport(String),
    BackoffExhausted(String),
    InvalidDispatchPolicy(String),
    NoAliveStore(String),
    MissingRegion(RegionVerId),
    RemoteReadLimit(String),
    OtherResponse(String),
    ServerTimeout,
    QueryInterrupted,
    Closed,
}

impl Display for BatchError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("request cancelled"),
            Self::ShuttingDown => f.write_str("TiDB is shutting down"),
            Self::Transport(message) => write!(f, "transport error: {message}"),
            Self::BackoffExhausted(message) => write!(f, "backoff exhausted: {message}"),
            Self::InvalidDispatchPolicy(policy) => {
                write!(f, "unexpected dispatch policy {policy}")
            }
            Self::NoAliveStore(message) => f.write_str(message),
            Self::MissingRegion(region) => write!(f, "region {region} is unavailable"),
            Self::RemoteReadLimit(message) | Self::OtherResponse(message) => f.write_str(message),
            Self::ServerTimeout => f.write_str("TiFlash server timeout"),
            Self::QueryInterrupted => f.write_str("query interrupted"),
            Self::Closed => f.write_str("batch coprocessor is closed"),
        }
    }
}

impl Error for BatchError {}

/// Batch Cop 操作的统一 Result 别名。
pub type BatchResult<T> = Result<T, BatchError>;

/// Region 版本标识：id + conf_ver + version，用于缓存失效与重定位。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RegionVerId {
    pub id: u64,
    pub conf_ver: u64,
    pub version: u64,
}

impl RegionVerId {
    pub const fn new(id: u64, conf_ver: u64, version: u64) -> Self {
        Self {
            id,
            conf_ver,
            version,
        }
    }
}

impl Display for RegionVerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.id, self.conf_ver, self.version)
    }
}

/// 半开区间 `[start, end)` 的键范围。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyRange {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

/// 多个 KeyRange 的集合，可按 start 排序。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyRanges(pub Vec<KeyRange>);

impl KeyRanges {
    pub fn new(ranges: Vec<KeyRange>) -> Self {
        Self(ranges)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &KeyRange> {
        self.0.iter()
    }

    /// 按各 range 的 start 键排序后返回。
    pub fn into_sorted(mut self) -> Self {
        self.0.sort_by(|left, right| left.start.cmp(&right.start));
        self
    }
}

/// Region 元信息：id 与 peer 列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionMeta {
    pub id: u64,
    pub peers: Vec<u64>,
}

/// Raft peer：副本 id 与所在 store id。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Peer {
    pub id: u64,
    pub store_id: u64,
}

/// 存储节点：id、地址与标签（如 engine=tiflash）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Store {
    pub id: u64,
    pub address: String,
    pub labels: HashMap<String, String>,
}

/// 单次 RPC 目标上下文：Region、地址、store/peer/meta。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RpcContext {
    pub region: RegionVerId,
    pub address: String,
    pub store: Option<Store>,
    pub meta: Option<RegionMeta>,
    pub peer: Option<Peer>,
}

/// 下推到协处理器的 Region 描述（含 ranges）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoprocessorRegionInfo {
    pub region_id: u64,
    pub conf_ver: u64,
    pub version: u64,
    pub ranges: Vec<KeyRange>,
}

/// 分区表物理表 id 与其下各 Region 列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableRegions {
    pub physical_table_id: i64,
    pub regions: Vec<CoprocessorRegionInfo>,
}

/// RegionInfo contains the same fields as the Go batch-coprocessor request model.
/// 与 Go batch-coprocessor 请求模型字段对齐：Region、Meta、Ranges、AllStores、PartitionIndex。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[allow(non_snake_case)]
pub struct RegionInfo {
    pub Region: RegionVerId,
    pub Meta: Option<RegionMeta>,
    pub Ranges: KeyRanges,
    pub AllStores: Vec<u64>,
    pub PartitionIndex: i64,
}

impl RegionInfo {
    pub fn to_coprocessor_region_info(&self) -> CoprocessorRegionInfo {
        CoprocessorRegionInfo {
            region_id: self.Region.id,
            conf_ver: self.Region.conf_ver,
            version: self.Region.version,
            ranges: self.Ranges.0.clone(),
        }
    }

    #[allow(non_snake_case)]
    pub fn toCoprocessorRegionInfo(&self) -> CoprocessorRegionInfo {
        self.to_coprocessor_region_info()
    }
}

/// Batch Cop RPC 命令类型。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CommandType {
    #[default]
    BatchCop,
}

/// 请求侧携带的 Region/Peer 上下文。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestContext {
    pub region: Option<RegionMeta>,
    pub peer: Option<Peer>,
}

/// 批量协处理器请求：命令、上下文与载荷。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchRequest {
    pub command: CommandType,
    pub context: RequestContext,
    pub payload: Vec<u8>,
}

/// 批量协处理器响应：数据、其他错误串与需重试的 Region 列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchResponse {
    pub data: Vec<u8>,
    pub other_error: String,
    pub retry_regions: Vec<RegionVerId>,
}

impl BatchResponse {
    pub fn encoded_size(&self) -> usize {
        self.data.len()
            + self.other_error.len()
            + self.retry_regions.len() * std::mem::size_of::<RegionVerId>()
    }
}

/// RPC 层返回的一批 BatchResponse（或错误）队列。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RpcResponse {
    pub responses: VecDeque<BatchResult<BatchResponse>>,
}

/// RPC 调用耗时统计（命令类型 + Duration）。
#[derive(Clone, Debug, Default)]
pub struct RpcRuntimeStats {
    pub calls: Vec<(CommandType, Duration)>,
}

/// 取消令牌：标记请求已被取消。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 向指定地址发送 BatchRequest 的 RPC 客户端抽象。
pub trait RpcClient: Send + Sync {
    fn send_request(
        &self,
        address: &str,
        request: &BatchRequest,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> BatchResult<RpcResponse>;
}

/// Region 发送失败回调：通知缓存失效/重建。
pub trait RegionFailureHandler: Send + Sync {
    fn on_send_fail_for_batch_regions(
        &self,
        store: Option<&Store>,
        regions: &[RegionInfo],
        reload_region: bool,
        error: &BatchError,
    );
}

/// 退避控制器：记录失败历史，超过 max_attempts 则报 BackoffExhausted。
#[derive(Clone, Debug)]
pub struct Backoffer {
    max_attempts: usize,
    attempts: usize,
    cancelled: CancellationToken,
    pub history: Vec<String>,
}

impl Backoffer {
    pub fn new(max_attempts: usize) -> Self {
        Self {
            max_attempts,
            attempts: 0,
            cancelled: CancellationToken::default(),
            history: Vec::new(),
        }
    }

    pub fn with_cancellation(max_attempts: usize, cancellation: CancellationToken) -> Self {
        Self {
            max_attempts,
            attempts: 0,
            cancelled: cancellation,
            history: Vec::new(),
        }
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.cancelled.clone()
    }

    /// 记录错误并递增尝试次数；取消或超限则返回对应错误。
    pub fn backoff(&mut self, error: &BatchError) -> BatchResult<()> {
        if self.cancelled.is_cancelled() || matches!(error, BatchError::Cancelled) {
            return Err(BatchError::Cancelled);
        }
        self.history.push(error.to_string());
        self.attempts += 1;
        if self.attempts > self.max_attempts {
            return Err(BatchError::BackoffExhausted(error.to_string()));
        }
        Ok(())
    }
}

/// 一次发送结果：响应、是否需重试、取消令牌与错误。
pub struct SendResult {
    pub response: Option<RpcResponse>,
    pub retry: bool,
    pub cancellation: CancellationToken,
    pub error: Option<BatchError>,
}

impl SendResult {
    fn failed(error: BatchError) -> Self {
        Self {
            response: None,
            retry: false,
            cancellation: CancellationToken::default(),
            error: Some(error),
        }
    }
}

/// Sends BatchCop requests and owns the retry decision for transport failures.
/// 发送 BatchCop 请求，并在传输失败时决定是否重试。
pub struct RegionBatchRequestSender {
    client: Arc<dyn RpcClient>,
    region_cache: Arc<dyn RegionFailureHandler>,
    enable_collect_execution_info: bool,
    disaggregated_tiflash: bool,
    shutting_down: Arc<AtomicBool>,
    pub stats: Option<Arc<Mutex<RpcRuntimeStats>>>,
    pub last_rpc_error: Option<BatchError>,
}

impl RegionBatchRequestSender {
    pub fn new(
        region_cache: Arc<dyn RegionFailureHandler>,
        client: Arc<dyn RpcClient>,
        enable_collect_execution_info: bool,
        disaggregated_tiflash: bool,
        shutting_down: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            region_cache,
            enable_collect_execution_info,
            disaggregated_tiflash,
            shutting_down,
            stats: None,
            last_rpc_error: None,
        }
    }

    /// 向 rpc_context.address 发送请求；失败时走 on_send_fail 并可能标记 retry。
    pub fn send_req_to_addr(
        &mut self,
        backoffer: &mut Backoffer,
        rpc_context: &RpcContext,
        region_infos: &[RegionInfo],
        request: &mut BatchRequest,
        timeout: Duration,
    ) -> SendResult {
        request.context = RequestContext {
            region: rpc_context.meta.clone(),
            peer: rpc_context.peer.clone(),
        };
        if rpc_context.address.is_empty() {
            return SendResult::failed(BatchError::Transport(
                "RPC context has no target address".to_owned(),
            ));
        }

        if backoffer.cancellation().is_cancelled() {
            return SendResult::failed(BatchError::Cancelled);
        }
        // I/O 失败后取消子 RPC，但父级 backoff 上下文仍可用于重建整批并重试。
        // The RPC child is cancelled after an I/O failure, but the parent backoff
        // context remains usable for rebuilding and retrying the complete batch.
        let cancellation = CancellationToken::default();

        let start = Instant::now();
        let result = if fail::eval(
            "github.com/pingcap/tidb/pkg/store/copr/mockBatchCopResponseError",
            |_| true,
        )
        .unwrap_or(false)
        {
            Err(BatchError::OtherResponse(
                "mock batch cop response error".to_owned(),
            ))
        } else {
            self.client
                .send_request(&rpc_context.address, request, timeout, &cancellation)
        };
        if self.enable_collect_execution_info
            && let Some(stats) = &self.stats
        {
            stats
                .lock()
                .expect("RPC statistics lock poisoned")
                .calls
                .push((request.command, start.elapsed()));
        }

        match result {
            Ok(response) => SendResult {
                response: Some(response),
                retry: false,
                cancellation,
                error: None,
            },
            Err(error) => {
                cancellation.cancel();
                self.last_rpc_error = Some(error.clone());
                match self.on_send_fail_for_batch_regions(
                    backoffer,
                    rpc_context,
                    region_infos,
                    &error,
                ) {
                    Ok(()) => SendResult {
                        response: None,
                        retry: true,
                        cancellation: CancellationToken::default(),
                        error: None,
                    },
                    Err(error) => SendResult::failed(error),
                }
            }
        }
    }

    /// 发送失败处理：非存算分离时通知 Region 缓存，再执行 backoff。
    pub fn on_send_fail_for_batch_regions(
        &mut self,
        backoffer: &mut Backoffer,
        rpc_context: &RpcContext,
        region_infos: &[RegionInfo],
        error: &BatchError,
    ) -> BatchResult<()> {
        if matches!(error, BatchError::Cancelled) {
            return Err(BatchError::Cancelled);
        }
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(BatchError::ShuttingDown);
        }

        if !self.disaggregated_tiflash {
            // 刻意恒为 true，与 Go「每次发送失败都重建」规则一致。
            // This is deliberately always true, matching the Go rebuild-on-every-send-failure rule.
            self.region_cache.on_send_fail_for_batch_regions(
                rpc_context.store.as_ref(),
                region_infos,
                true,
                error,
            );
        }
        backoffer.backoff(error)
    }
}

/// Go 风格构造函数：返回堆上的 `RegionBatchRequestSender`。
#[allow(non_snake_case)]
pub fn NewRegionBatchRequestSender(
    region_cache: Arc<dyn RegionFailureHandler>,
    client: Arc<dyn RpcClient>,
    enable_collect_execution_info: bool,
    disaggregated_tiflash: bool,
    shutting_down: Arc<AtomicBool>,
) -> Box<RegionBatchRequestSender> {
    Box::new(RegionBatchRequestSender::new(
        region_cache,
        client,
        enable_collect_execution_info,
        disaggregated_tiflash,
        shutting_down,
    ))
}

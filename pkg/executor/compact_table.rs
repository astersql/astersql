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

// ALTER TABLE ... COMPACT 执行器（TiFlash 路径）：向各 TiFlash store 发起分页压缩。
//
// Compact（压缩/整理）用于回收空间或整理列存；按物理表（分区）分页发送 RPC，
// 支持网络错误退避重试、进度日志与查询取消。无 TiFlash 副本时跳过并告警。
#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use astersql_errors as errors;

/// 适配层通用结果类型。
pub type AdapterResult<T = ()> = Result<T, errors::SharedError>;

/// 单次 compact RPC 超时（1 小时）。
pub const compactRequestTimeout: Duration = Duration::from_secs(60 * 60);

/// 网络错误退避的最大睡眠毫秒数。
pub const compactMaxBackoffSleepMs: u64 = 5 * 1000;

/// 进度日志最小间隔。
pub const compactProgressReportInterval: Duration = Duration::from_secs(10);

/// Compact 语句输出 chunk（无结果列，仅 Reset）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactChunk;

impl CompactChunk {
    /// 清空输出缓冲。
    pub fn Reset(&mut self) {}
}

/// 集群节点角色类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerType {
    TiKV,
    TiFlash,
    TiDB,
    Unknown(String),
}

/// 单个 store 的类型与地址。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerInfo {
    /// 节点类型。
    pub ServerType: ServerType,
    /// 访问地址。
    pub Address: String,
}

/// TiFlash 副本信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TiFlashReplicaInfo {
    /// 副本数量；为 0 时跳过 compact。
    pub Count: u64,
}

/// 分区定义。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    /// 分区物理表 ID。
    pub ID: i64,
}

/// 分区信息集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionInfo {
    /// 分区定义列表。
    pub Definitions: Vec<PartitionDefinition>,
}

/// 待压缩的逻辑表元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    /// 逻辑表 ID。
    pub ID: i64,
    /// 表名。
    pub Name: String,
    /// TiFlash 副本；无或 Count=0 则跳过。
    pub TiFlashReplica: Option<TiFlashReplicaInfo>,
    /// 分区信息。
    pub Partition: Option<PartitionInfo>,
}

/// TiFlash compact 业务错误种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactErrorKind {
    /// 目标表正在压缩中。
    CompactInProgress,
    /// store 待处理任务过多。
    TooManyPendingTasks,
    /// 物理表不存在（可跳过）。
    PhysicalTableNotExist,
    /// 未知内部错误。
    Unknown,
}

/// Compact 响应中的业务错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactError {
    /// 错误种类。
    pub Kind: CompactErrorKind,
    /// 错误消息。
    pub Message: String,
}

impl CompactError {
    /// 返回错误种类。
    pub fn kind(&self) -> CompactErrorKind {
        self.Kind
    }
}

/// 发往 TiFlash 的 compact 请求。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactRequest {
    /// 逻辑表 ID。
    pub LogicalTableId: i64,
    /// 物理表/分区 ID。
    pub PhysicalTableId: i64,
    /// 本页起始键；空表示从头开始。
    pub StartKey: Vec<u8>,
}

/// TiFlash compact 单页响应。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactResponse {
    /// 业务错误（若有）。
    pub Error: Option<CompactError>,
    /// 是否仍有后续页。
    pub HasRemaining: bool,
    /// 本页已压缩起始键。
    pub CompactedStartKey: Vec<u8>,
    /// 本页已压缩结束键（下一页 StartKey）。
    pub CompactedEndKey: Vec<u8>,
}

impl CompactResponse {
    /// 取得业务错误引用。
    pub fn GetError(&self) -> Option<&CompactError> {
        self.Error.as_ref()
    }

    /// 本页压缩起始键。
    pub fn GetCompactedStartKey(&self) -> &[u8] {
        &self.CompactedStartKey
    }

    /// 本页压缩结束键。
    pub fn GetCompactedEndKey(&self) -> &[u8] {
        &self.CompactedEndKey
    }
}

/// RPC 层响应包装。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactRpcResponse {
    /// 响应体；缺失视为传输/协议错误。
    pub body: Option<CompactResponse>,
}

/// 传输层错误种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactTransportErrorKind {
    /// 查询取消。
    Cancelled,
    /// 截止时间超时。
    DeadlineExceeded,
    /// gRPC 取消。
    GrpcCancelled,
    /// 可重试的网络错误。
    Network,
}

/// Compact RPC 传输错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactTransportError {
    pub kind: CompactTransportErrorKind,
    pub message: String,
}

impl CompactTransportError {
    /// 仅 Network 类错误可退避重试。
    fn retryable(&self) -> bool {
        self.kind == CompactTransportErrorKind::Network
    }

    /// 转为共享错误类型。
    fn into_error(self) -> errors::SharedError {
        errors::New(self.message)
    }
}

/// 日志上下文：表、分区与目标 store。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactLogContext {
    pub table: String,
    pub table_id: i64,
    pub requested_partition_ids: Vec<i64>,
    pub store_address: String,
}

/// Compact 过程日志事件。
#[derive(Clone, Debug, PartialEq)]
pub enum CompactLogEvent {
    Begin {
        context: CompactLogContext,
    },
    Finished {
        context: CompactLogContext,
        elapsed: Duration,
    },
    Progress {
        context: CompactLogContext,
        compacted_ratio: f64,
        elapsed: Duration,
        all_physical_tables: usize,
        compacted_physical_tables: usize,
    },
    Failure {
        context: CompactLogContext,
        physical_table_id: i64,
        detail: String,
    },
    PhysicalTableSkipped {
        context: CompactLogContext,
        physical_table_id: i64,
        response_error: String,
    },
    InvalidPage {
        context: CompactLogContext,
        physical_table_id: i64,
        compacted_start_key_hex: String,
        compacted_end_key_hex: String,
    },
}

/// 单次语句作用域的 RPC 运行：取消、发送与资源释放。
/// A single statement-scoped RPC run. Implementations must bind query kill/deadline
/// cancellation to `IsCancelled`, cancel every in-flight RPC in `Cancel`, and release
/// all run-scoped resources in `Finish`.
pub trait CompactRun: Send + Sync {
    fn IsCancelled(&self) -> bool;
    fn CancellationError(&self) -> errors::SharedError;
    fn Cancel(&self);
    fn SendCompact(
        &self,
        address: &str,
        request: &CompactRequest,
        timeout: Duration,
    ) -> Result<CompactRpcResponse, CompactTransportError>;
    fn Finish(&self) -> AdapterResult;
}

/// 单页请求的有界退避状态。
/// The bounded retry state for exactly one compact page request.
pub trait CompactBackoff: Send {
    fn Backoff(&mut self, network_error: &str) -> AdapterResult;
}

/// 生产边界：发现 store、告警、日志、RPC 生命周期与重试策略。
/// Production boundary for discovery, warnings, logging, RPC lifecycle, and retry policy.
/// Every operation is mandatory; there are no default-success implementations.
pub trait CompactRuntime: Send + Sync {
    fn GetStoreServerInfo(&self) -> AdapterResult<Vec<ServerInfo>>;
    fn AppendWarning(&self, warning: &str);
    fn Log(&self, event: CompactLogEvent);
    fn BeginRun(&self) -> AdapterResult<Arc<dyn CompactRun>>;
    fn NewBackoff(&self, max_sleep_ms: u64) -> Box<dyn CompactBackoff>;
}

/// 从集群信息中筛选 TiFlash store 列表。
pub fn getTiFlashStores(runtime: &dyn CompactRuntime) -> AdapterResult<Vec<ServerInfo>> {
    Ok(runtime
        .GetStoreServerInfo()?
        .into_iter()
        .filter(|store| store.ServerType == ServerType::TiFlash)
        .collect())
}

/// 面向 TiFlash 的 COMPACT TABLE 执行器。
pub struct CompactTableTiFlashExec {
    /// 运行时依赖。
    pub runtime: Arc<dyn CompactRuntime>,
    /// 目标表信息。
    pub tableInfo: TableInfo,
    /// 指定分区 ID；空表示全部或非分区。
    pub partitionIDs: Vec<i64>,
    /// 是否已执行（只跑一次）。
    pub done: bool,
}

impl CompactTableTiFlashExec {
    /// 执行 compact；无输出行，完成后标记 done。
    pub fn Next(&mut self, output: &mut CompactChunk) -> AdapterResult {
        output.Reset();
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.doCompact()
    }

    /// 向各 TiFlash store 并行发起压缩；聚合取消与 panic。
    pub fn doCompact(&mut self) -> AdapterResult {
        // 无 TiFlash 副本则跳过。
        if self
            .tableInfo
            .TiFlashReplica
            .as_ref()
            .is_none_or(|replica| replica.Count == 0)
        {
            self.runtime
                .AppendWarning("compact skipped: no tiflash replica in the table");
            return Ok(());
        }

        let stores = getTiFlashStores(self.runtime.as_ref())?;
        let run = self.runtime.BeginRun()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let table_info = Arc::new(self.tableInfo.clone());
        let partition_ids = Arc::new(self.partitionIDs.clone());
        let runtime = Arc::clone(&self.runtime);

        // 每 store 一个 worker；任一 stop_all 或 panic 则取消全局。
        let workers = std::thread::scope(|scope| {
            let mut workers = Vec::with_capacity(stores.len());
            for store in stores {
                let task = storeCompactTask {
                    runtime: Arc::clone(&runtime),
                    run: Arc::clone(&run),
                    cancelled: Arc::clone(&cancelled),
                    tableInfo: Arc::clone(&table_info),
                    partitionIDs: Arc::clone(&partition_ids),
                    targetStore: store,
                    startAt: Instant::now(),
                    allPhysicalTables: 0,
                    compactedPhysicalTables: 0,
                    lastProgressOutputAt: Instant::now(),
                };
                workers.push(scope.spawn(move || task.work()));
            }

            let mut panic_detected = false;
            for worker in workers {
                match worker.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if error.stop_all {
                            cancelled.store(true, Ordering::Release);
                            run.Cancel();
                        }
                    }
                    Err(_) => {
                        panic_detected = true;
                        cancelled.store(true, Ordering::Release);
                        run.Cancel();
                    }
                }
            }
            panic_detected
        });

        let finish = run.Finish();
        if workers {
            let _ = finish;
            return Err(errors::New("TiFlash compact worker panicked"));
        }
        finish
    }
}

/// 单个 TiFlash store 上的压缩任务状态。
pub struct storeCompactTask {
    pub runtime: Arc<dyn CompactRuntime>,
    pub run: Arc<dyn CompactRun>,
    pub cancelled: Arc<AtomicBool>,
    pub tableInfo: Arc<TableInfo>,
    pub partitionIDs: Arc<Vec<i64>>,
    pub targetStore: ServerInfo,
    pub startAt: Instant,
    pub allPhysicalTables: usize,
    pub compactedPhysicalTables: usize,
    pub lastProgressOutputAt: Instant,
}

/// store 任务错误：stop_all 表示需取消其他 store。
pub struct CompactTaskError {
    /// 是否停止全部 store 上的任务。
    pub stop_all: bool,
    /// 原始错误。
    pub source: errors::SharedError,
}

impl storeCompactTask {
    /// 在本 store 上依次压缩各物理表。
    pub fn work(mut self) -> Result<(), CompactTaskError> {
        self.startAt = Instant::now();
        self.lastProgressOutputAt = self.startAt;
        self.runtime.Log(CompactLogEvent::Begin {
            context: self.log_context(),
        });

        // 分区表：未指定分区则压缩全部定义；否则仅指定分区。
        let physical_tables = if let Some(partition_info) = self.tableInfo.Partition.as_ref() {
            if self.partitionIDs.is_empty() {
                partition_info
                    .Definitions
                    .iter()
                    .map(|definition| definition.ID)
                    .collect::<Vec<_>>()
            } else {
                self.partitionIDs.as_ref().clone()
            }
        } else {
            vec![self.tableInfo.ID]
        };
        self.allPhysicalTables = physical_tables.len();
        self.compactedPhysicalTables = 0;

        for physical_table_id in physical_tables {
            let result = self.compactOnePhysicalTable(physical_table_id);
            self.compactedPhysicalTables += 1;
            if let Err(error) = result {
                if error.stop_all {
                    self.cancelled.store(true, Ordering::Release);
                    self.run.Cancel();
                    return Err(error);
                }
                // Store-local failures are already warnings. They stop the remaining
                // partitions for this store but do not fail the errgroup.
                return Ok(());
            }
        }

        self.runtime.Log(CompactLogEvent::Finished {
            context: self.log_context(),
            elapsed: self.startAt.elapsed(),
        });
        Ok(())
    }

    /// 记录单物理表失败日志。
    pub fn logFailure(&self, physical_table_id: i64, detail: String) {
        self.runtime.Log(CompactLogEvent::Failure {
            context: self.log_context(),
            physical_table_id,
            detail,
        });
    }

    /// 按间隔输出压缩进度日志。
    pub fn logProgressOptionally(&mut self) {
        if self.lastProgressOutputAt.elapsed() <= compactProgressReportInterval {
            return;
        }
        self.lastProgressOutputAt = Instant::now();
        let ratio = if self.allPhysicalTables == 0 {
            0.0
        } else {
            self.compactedPhysicalTables as f64 / self.allPhysicalTables as f64
        };
        self.runtime.Log(CompactLogEvent::Progress {
            context: self.log_context(),
            compacted_ratio: ratio,
            elapsed: self.startAt.elapsed(),
            all_physical_tables: self.allPhysicalTables,
            compacted_physical_tables: self.compactedPhysicalTables,
        });
    }

    /// 对单个物理表分页 compact，直到无剩余或出错。
    pub fn compactOnePhysicalTable(
        &mut self,
        physical_table_id: i64,
    ) -> Result<(), CompactTaskError> {
        let mut start_key = Vec::new();
        // 分页循环：用上一页 CompactedEndKey 作为下一页 StartKey。
        loop {
            if self.cancelled.load(Ordering::Acquire) || self.run.IsCancelled() {
                return Err(CompactTaskError {
                    stop_all: true,
                    source: self.run.CancellationError(),
                });
            }
            self.logProgressOptionally();

            let request = CompactRequest {
                LogicalTableId: self.tableInfo.ID,
                PhysicalTableId: physical_table_id,
                StartKey: start_key,
            };
            let response = match self.sendRequestWithRetry(&request) {
                Ok(response) => response,
                Err(error) => {
                    let warning = format!(
                        "compact on store {} failed: {}",
                        self.targetStore.Address, error
                    );
                    self.runtime.AppendWarning(&warning);
                    self.logFailure(physical_table_id, error.to_string());
                    return Err(CompactTaskError {
                        stop_all: false,
                        source: errors::New(warning),
                    });
                }
            };

            // 按业务错误种类决定：全局取消 / 本 store 停止 / 跳过物理表。
            if let Some(response_error) = response.GetError() {
                match response_error.kind() {
                    CompactErrorKind::CompactInProgress => {
                        let error = self.responseFailure(
                            physical_table_id,
                            true,
                            "table is compacting in progress",
                            response_error,
                        );
                        self.cancelled.store(true, Ordering::Release);
                        self.run.Cancel();
                        return Err(error);
                    }
                    CompactErrorKind::TooManyPendingTasks => {
                        return Err(self.responseFailure(
                            physical_table_id,
                            false,
                            "store is too busy",
                            response_error,
                        ));
                    }
                    CompactErrorKind::PhysicalTableNotExist => {
                        self.runtime.Log(CompactLogEvent::PhysicalTableSkipped {
                            context: self.log_context(),
                            physical_table_id,
                            response_error: response_error.Message.clone(),
                        });
                        return Ok(());
                    }
                    CompactErrorKind::Unknown => {
                        return Err(self.responseFailure(
                            physical_table_id,
                            false,
                            "internal error (check logs for details)",
                            response_error,
                        ));
                    }
                }
            }

            if !response.HasRemaining {
                return Ok(());
            }
            let end_key = response.GetCompactedEndKey();
            if end_key.is_empty() {
                let warning = format!(
                    "compact on store {} failed: internal error (check logs for details)",
                    self.targetStore.Address
                );
                self.runtime.AppendWarning(&warning);
                self.runtime.Log(CompactLogEvent::InvalidPage {
                    context: self.log_context(),
                    physical_table_id,
                    compacted_start_key_hex: hex_encode(response.GetCompactedStartKey()),
                    compacted_end_key_hex: hex_encode(end_key),
                });
                return Err(CompactTaskError {
                    stop_all: false,
                    source: errors::New(warning),
                });
            }
            start_key = end_key.to_vec();
        }
    }

    /// 将响应错误转为任务错误，并写告警与失败日志。
    fn responseFailure(
        &self,
        physical_table_id: i64,
        stop_all: bool,
        reason: &str,
        response_error: &CompactError,
    ) -> CompactTaskError {
        let warning = format!(
            "compact on store {} failed: {}",
            self.targetStore.Address, reason
        );
        self.runtime.AppendWarning(&warning);
        self.logFailure(
            physical_table_id,
            format!("{}: {}", response_error.Message, reason),
        );
        CompactTaskError {
            stop_all,
            source: errors::New(warning),
        }
    }

    /// 发送请求；对可重试网络错误做退避循环。
    pub fn sendRequestWithRetry(
        &mut self,
        request: &CompactRequest,
    ) -> AdapterResult<CompactResponse> {
        let mut backoff = self.runtime.NewBackoff(compactMaxBackoffSleepMs);
        loop {
            if self.cancelled.load(Ordering::Acquire) || self.run.IsCancelled() {
                return Err(self.run.CancellationError());
            }
            match self
                .run
                .SendCompact(&self.targetStore.Address, request, compactRequestTimeout)
            {
                Ok(response) => {
                    return response
                        .body
                        .ok_or_else(|| errors::New("TiFlash compact response body is missing"));
                }
                Err(error) if !error.retryable() => return Err(error.into_error()),
                Err(error) => {
                    let message = error.message;
                    if backoff.Backoff(&message).is_err() {
                        return Err(errors::New(message));
                    }
                }
            }
        }
    }

    /// 构造当前任务的日志上下文。
    fn log_context(&self) -> CompactLogContext {
        CompactLogContext {
            table: self.tableInfo.Name.clone(),
            table_id: self.tableInfo.ID,
            requested_partition_ids: self.partitionIDs.as_ref().clone(),
            store_address: self.targetStore.Address.clone(),
        }
    }
}

/// 将字节编码为小写十六进制（用于无效页日志）。
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

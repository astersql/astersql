// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// MPP（Massively Parallel Processing，大规模并行处理）客户端。
//
// 将 SQL 查询切分为可在 TiFlash 等计算节点上并行执行的 MPP 任务：
// 构造按 Region 划分的 batch cop 任务、下发执行计划（plan）、取消查询、
// 建立任务间数据流连接，并缓存可用 MPP store 数量。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::batch_coprocessor::{
    BatchBuildOptions, BatchTaskSource, DispatchPolicy, ReplicaReadPolicy,
    TI_FLASH_READ_TIMEOUT_ULTRA_LONG, batchCopTask,
    build_batch_cop_tasks_for_non_partitioned_table, build_batch_cop_tasks_for_partitioned_table,
};
use crate::batch_request_sender::{
    Backoffer, BatchError, BatchResult, CoprocessorRegionInfo, KeyRange, KeyRanges, RegionVerId,
    Store, TableRegions,
};

/// 下发 MPP 任务时的中等读超时（60s）。
const READ_TIMEOUT_MEDIUM: Duration = Duration::from_secs(60);
/// 取消 MPP 任务时的短超时（5s）。
const READ_TIMEOUT_SHORT: Duration = Duration::from_secs(5);
/// MPP store 数量缓存的存活时间（微秒，约 120 秒）。
const MPP_STORE_COUNT_TTL_MICROS: i64 = 120_000_000;

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
/// 全局唯一的 MPP 查询标识：时间戳、本地查询 ID 与 server ID。
pub struct MppQueryId {
    /// 查询开始时间戳（用于组成查询身份）。
    pub query_ts: u64,
    /// 本节点内的查询序号。
    pub local_query_id: u64,
    /// 发起查询的 TiDB server 标识。
    pub server_id: u64,
}

#[derive(Clone, Debug, Default)]
/// 分区表某一物理分区的 ID 及其键范围列表。
pub struct PartitionRanges {
    /// 物理分区 ID。
    pub id: i64,
    /// 该分区上需要扫描的键范围。
    pub ranges: Vec<KeyRange>,
}

#[derive(Clone, Debug, Default)]
/// 构造 MPP/batch cop 任务的请求：起始时间戳与键范围（或分区范围）。
pub struct MppBuildTasksRequest {
    /// 快照读起始时间戳（start_ts，MVCC 可见性边界）。
    pub start_ts: u64,
    /// 非分区表的键范围；与 partition_id_and_ranges 二选一。
    pub key_ranges: Option<Vec<KeyRange>>,
    /// 分区表各分区的 ID 与键范围。
    pub partition_id_and_ranges: Option<Vec<PartitionRanges>>,
}

#[derive(Clone, Debug, Default)]
/// 单个 MPP 任务的元数据：查询身份、协调者地址、资源组与执行摘要开关等。
pub struct MppTaskMeta {
    /// 快照读起始时间戳。
    pub start_ts: u64,
    /// 查询时间戳分量。
    pub query_ts: u64,
    /// 本地查询 ID 分量。
    pub local_query_id: u64,
    /// 任务在查询内的编号；建立连接时接收端可为 -1。
    pub task_id: i64,
    /// 发起查询的 server 标识。
    pub server_id: u64,
    /// Gather（汇总）阶段标识。
    pub gather_id: u64,
    /// 任务所在 store 的地址。
    pub address: String,
    /// MPP 协调者（coordinator）地址。
    pub coordinator_address: String,
    /// 是否上报执行摘要（execution summary）。
    pub report_execution_summary: bool,
    /// MPP 协议版本。
    pub mpp_version: i64,
    /// 资源组名称（Resource Control）。
    pub resource_group_name: String,
    /// 会话连接 ID。
    pub connection_id: u64,
    /// 连接别名。
    pub connection_alias: String,
    /// SQL 文本摘要（digest）。
    pub sql_digest: Vec<u8>,
    /// 执行计划摘要。
    pub plan_digest: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
/// 下发单个 MPP 任务的高层请求，含编码计划与目标 store 信息。
pub struct DispatchMppTaskRequest {
    /// 快照读起始时间戳。
    pub start_ts: u64,
    /// 所属 MPP 查询身份。
    pub query_id: MppQueryId,
    /// 任务 ID。
    pub id: i64,
    /// Gather 阶段标识。
    pub gather_id: u64,
    /// 关联的 batch cop 任务元数据（含 Region 信息）；无 Region 时可为空。
    pub meta: Option<batchCopTask>,
    /// 目标 store 地址（meta 缺失时使用）。
    pub address: String,
    /// 协调者地址。
    pub coordinator_address: String,
    /// 是否上报执行摘要。
    pub report_execution_summary: bool,
    /// MPP 协议版本。
    pub mpp_version: i64,
    /// 资源组名称。
    pub resource_group_name: String,
    /// 会话连接 ID。
    pub connection_id: u64,
    /// 连接别名。
    pub connection_alias: String,
    /// SQL 摘要。
    pub sql_digest: Vec<u8>,
    /// 计划摘要。
    pub plan_digest: Vec<u8>,
    /// 编码后的执行计划片段。
    pub data: Vec<u8>,
    /// schema 版本，用于存储端校验元数据一致性。
    pub schema_version: i64,
}

impl DispatchMppTaskRequest {
    /// 优先取 batchCopTask 地址，否则用请求自身 address。
    fn address(&self) -> &str {
        self.meta
            .as_ref()
            .map(batchCopTask::get_address)
            .unwrap_or(&self.address)
    }
}

#[derive(Clone, Debug, Default)]
/// 发往 store 的线格式下发请求：任务元数据、编码计划与 Region 列表。
pub struct MppDispatchWireRequest {
    pub meta: MppTaskMeta,
    /// 编码后的计划。
    pub encoded_plan: Vec<u8>,
    /// 线协议超时秒数。
    pub timeout_seconds: u64,
    /// schema 版本。
    pub schema_version: i64,
    /// 非分区表场景下的 Region 信息列表。
    pub regions: Vec<CoprocessorRegionInfo>,
    /// 分区表场景下的按表 Region 分组；非空时优先于 regions。
    pub table_regions: Vec<TableRegions>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 下发响应：可选错误与需重试/失效的 Region 版本号。
pub struct MppDispatchResponse {
    /// 存储端返回的错误信息。
    pub error: Option<String>,
    /// 需要使本地缓存失效并重试的 Region。
    pub retry_regions: Vec<RegionVerId>,
}

#[derive(Clone, Debug, Default)]
/// 取消 MPP 任务的请求，携带任务元数据。
pub struct MppCancelRequest {
    pub meta: MppTaskMeta,
}

#[derive(Clone, Debug, Default)]
/// 建立发送端与接收端之间 MPP 数据通道的请求。
pub struct MppConnectionRequest {
    /// 发送端任务元数据。
    pub sender_meta: MppTaskMeta,
    /// 接收端任务元数据。
    pub receiver_meta: MppTaskMeta,
}

#[derive(Clone, Debug, Default)]
/// MPP 数据流：可关闭标志与已收到的数据包缓冲。
pub struct MppStream {
    /// 流是否已关闭。
    closed: Arc<AtomicBool>,
    /// 缓冲的数据包。
    pub packets: Vec<Vec<u8>>,
}

impl MppStream {
    /// 标记流已关闭。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// 查询流是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// MPP 传输抽象：下发/取消/建连、可见性检查、列举 store 与失效缓存。
pub trait MppTransport: Send + Sync + 'static {
    fn dispatch(
        &self,
        address: &str,
        request: &MppDispatchWireRequest,
        timeout: Duration,
    ) -> BatchResult<MppDispatchResponse>;
    fn cancel(
        &self,
        address: &str,
        request: &MppCancelRequest,
        timeout: Duration,
    ) -> BatchResult<()>;
    fn establish(
        &self,
        address: &str,
        request: &MppConnectionRequest,
        timeout: Duration,
    ) -> BatchResult<MppStream>;
    fn check_visibility(&self, start_ts: u64) -> BatchResult<()>;
    fn all_stores(&self) -> BatchResult<Vec<Store>>;
    fn invalidate_region(&self, region: RegionVerId);
    fn invalidate_compute_stores(&self);
}

#[derive(Default)]
/// 带 TTL 的 MPP store 数量缓存，避免频繁列举全部 store。
pub struct MppStoreCount {
    /// 缓存的可用 store 数量。
    count: AtomicI32,
    /// 上次刷新时间（微秒）。
    last_update: AtomicI64,
    /// 是否已成功初始化过缓存。
    initialized: AtomicBool,
}

impl MppStoreCount {
    /// 当前 Unix 微秒时间戳。
    fn now_micros() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
            .min(i64::MAX as u128) as i64
    }

    /// 在 TTL 内返回缓存值；否则竞争刷新，过滤 engine_role=write 的 store。
    pub fn get(
        &self,
        ttl_micros: i64,
        fetch: impl FnOnce() -> BatchResult<Vec<Store>>,
    ) -> BatchResult<usize> {
        let last = self.last_update.load(Ordering::Acquire);
        let now = Self::now_micros();
        let initialized = self.initialized.load(Ordering::Acquire);
        if now.saturating_sub(last) < ttl_micros && initialized {
            return Ok(self.count.load(Ordering::Acquire).max(0) as usize);
        }
        let won_refresh = self
            .last_update
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if !won_refresh && initialized {
            return Ok(self.count.load(Ordering::Acquire).max(0) as usize);
        }
        let stores = match fetch() {
            Ok(stores) => stores,
            Err(error) => {
                self.initialized.store(false, Ordering::Release);
                return Err(error);
            }
        };
        let count = stores
            .iter()
            .filter(|store| {
                !store
                    .labels
                    .get("engine_role")
                    .is_some_and(|role| role == "write")
            })
            .count();
        if !initialized || self.last_update.load(Ordering::Acquire) == now {
            self.count.store(count as i32, Ordering::Release);
            self.initialized.store(true, Ordering::Release);
        }
        Ok(count)
    }
}

/// MPP 客户端：构造任务、下发、取消、建连与查询 store 数量。
pub struct MppClient {
    /// 按键范围拆分 Region 任务的数据源。
    source: Arc<dyn BatchTaskSource>,
    /// 底层 MPP RPC 传输。
    transport: Arc<dyn MppTransport>,
    /// 是否为存算分离的 TiFlash Compute 拓扑。
    disaggregated_tiflash: bool,
    /// 是否使用自动扩缩容（影响 PD 失效策略）。
    use_auto_scaler: bool,
    /// store 数量缓存。
    store_count: MppStoreCount,
}

impl MppClient {
    /// 组装 MPP 客户端。
    pub fn new(
        source: Arc<dyn BatchTaskSource>,
        transport: Arc<dyn MppTransport>,
        disaggregated_tiflash: bool,
        use_auto_scaler: bool,
    ) -> Self {
        Self {
            source,
            transport,
            disaggregated_tiflash,
            use_auto_scaler,
            store_count: MppStoreCount::default(),
        }
    }

    /// 按分区或非分区键范围构造 MPP batch cop 任务列表。
    pub fn construct_mpp_tasks(
        &self,
        request: MppBuildTasksRequest,
        ttl: Duration,
        dispatch_policy: DispatchPolicy,
        replica_read_policy: ReplicaReadPolicy,
        append_warning: &mut dyn FnMut(BatchError),
    ) -> BatchResult<Vec<batchCopTask>> {
        let options = BatchBuildOptions {
            disaggregated_tiflash: self.disaggregated_tiflash,
            use_auto_scaler: self.use_auto_scaler,
            is_mpp: true,
            ttl,
            balance_with_continuity: true,
            balance_continuous_region_count: 20,
            dispatch_policy,
            replica_read_policy,
            ..BatchBuildOptions::default()
        };
        let mut backoffer = Backoffer::new(options.max_retries);
        if let Some(partitions) = request.partition_id_and_ranges {
            let ids = partitions
                .iter()
                .map(|partition| partition.id)
                .collect::<Vec<_>>();
            let ranges = partitions
                .into_iter()
                .map(|partition| KeyRanges::new(partition.ranges))
                .collect();
            build_batch_cop_tasks_for_partitioned_table(
                self.source.as_ref(),
                ranges,
                &ids,
                &options,
                &mut backoffer,
                append_warning,
            )
        } else {
            let ranges = request.key_ranges.ok_or_else(|| {
                BatchError::OtherResponse("KeyRanges in MPPBuildTasksRequest is nil".to_owned())
            })?;
            build_batch_cop_tasks_for_non_partitioned_table(
                self.source.as_ref(),
                KeyRanges::new(ranges),
                &options,
                &mut backoffer,
                append_warning,
            )
        }
    }

    /// 从下发请求提取任务元数据。
    fn task_meta(request: &DispatchMppTaskRequest) -> MppTaskMeta {
        MppTaskMeta {
            start_ts: request.start_ts,
            query_ts: request.query_id.query_ts,
            local_query_id: request.query_id.local_query_id,
            task_id: request.id,
            server_id: request.query_id.server_id,
            gather_id: request.gather_id,
            address: request.address().to_owned(),
            coordinator_address: request.coordinator_address.clone(),
            report_execution_summary: request.report_execution_summary,
            mpp_version: request.mpp_version,
            resource_group_name: request.resource_group_name.clone(),
            connection_id: request.connection_id,
            connection_alias: request.connection_alias.clone(),
            sql_digest: request.sql_digest.clone(),
            plan_digest: request.plan_digest.clone(),
        }
    }

    /// 下发 MPP 任务；失败时按策略退避或使 compute store 缓存失效。
    pub fn dispatch_mpp_task(
        &self,
        request: &DispatchMppTaskRequest,
        backoffer: &mut Backoffer,
    ) -> (Option<MppDispatchResponse>, bool, Option<BatchError>) {
        let meta = Self::task_meta(request);
        let mut regions: Vec<CoprocessorRegionInfo> = request
            .meta
            .as_ref()
            .map(|task| {
                task.regionInfos
                    .iter()
                    .map(|region| region.to_coprocessor_region_info())
                    .collect()
            })
            .unwrap_or_default();
        let table_regions = request
            .meta
            .as_ref()
            .map(|task| task.PartitionTableRegions.clone())
            .unwrap_or_default();
        // 分区表路径用 table_regions，清空普通 regions 避免重复。
        if !table_regions.is_empty() {
            regions.clear();
        }
        let wire = MppDispatchWireRequest {
            meta,
            encoded_plan: request.data.clone(),
            timeout_seconds: 60,
            schema_version: request.schema_version,
            regions,
            table_regions,
        };
        let result = self
            .transport
            .dispatch(request.address(), &wire, READ_TIMEOUT_MEDIUM);
        let invalid_pd = self.disaggregated_tiflash && !self.use_auto_scaler;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                if invalid_pd {
                    self.transport.invalidate_compute_stores();
                }
                // 带 Region 的任务无法本地重试：计划片段需重新切分调度。
                // Region-bearing tasks cannot be locally retried because the plan
                // fragments would need to be cut and scheduled again.
                if request.meta.is_some() || matches!(error, BatchError::Cancelled) {
                    return (None, false, Some(error));
                }
                let retry = backoffer.backoff(&error).is_ok();
                return (None, retry, Some(error));
            }
        };
        for region in &response.retry_regions {
            self.transport.invalidate_region(*region);
        }
        (Some(response), false, None)
    }

    /// 向相关 store 并行发送取消；每个查询身份只广播一次。
    pub fn cancel_mpp_tasks(
        &self,
        store_addresses: &HashSet<String>,
        requests: &[DispatchMppTaskRequest],
    ) {
        let Some(first) = requests.first() else {
            return;
        };
        if store_addresses.is_empty() {
            return;
        }
        // Go builds a dedicated cancel meta rather than reusing dispatch meta.
        let cancel = MppCancelRequest {
            meta: MppTaskMeta {
                start_ts: first.start_ts,
                gather_id: first.gather_id,
                query_ts: first.query_id.query_ts,
                local_query_id: first.query_id.local_query_id,
                server_id: first.query_id.server_id,
                mpp_version: first.mpp_version,
                resource_group_name: first.resource_group_name.clone(),
                sql_digest: first.sql_digest.clone(),
                plan_digest: first.plan_digest.clone(),
                ..MppTaskMeta::default()
            },
        };
        let got_error = Arc::new(AtomicBool::new(false));
        thread::scope(|scope| {
            for address in store_addresses {
                let got_error = Arc::clone(&got_error);
                let transport = Arc::clone(&self.transport);
                let cancel = cancel.clone();
                scope.spawn(move || {
                    if transport
                        .cancel(address, &cancel, READ_TIMEOUT_SHORT)
                        .is_err()
                    {
                        got_error.store(true, Ordering::Release);
                    }
                });
            }
        });
        if self.disaggregated_tiflash && !self.use_auto_scaler && got_error.load(Ordering::Acquire)
        {
            self.transport.invalidate_compute_stores();
        }
    }

    /// 建立到目标 store 的 MPP 数据流；接收端 task_id 置为 -1。
    pub fn establish_mpp_connection(
        &self,
        request: &DispatchMppTaskRequest,
        sender_meta: MppTaskMeta,
        backoffer: &mut Backoffer,
    ) -> (Option<MppStream>, bool, Option<BatchError>) {
        let connection = MppConnectionRequest {
            sender_meta,
            receiver_meta: MppTaskMeta {
                task_id: -1,
                ..Self::task_meta(request)
            },
        };
        match self.transport.establish(
            request.address(),
            &connection,
            TI_FLASH_READ_TIMEOUT_ULTRA_LONG,
        ) {
            Ok(stream) => (Some(stream), false, None),
            Err(error) => {
                if self.disaggregated_tiflash && !self.use_auto_scaler {
                    self.transport.invalidate_compute_stores();
                }
                if matches!(error, BatchError::Cancelled) {
                    return (None, false, Some(error));
                }
                let retry = backoffer.backoff(&error).is_ok();
                (None, retry, Some(error))
            }
        }
    }

    /// 检查 start_ts 在存储端的可见性（GC / safepoint）。
    pub fn check_visibility(&self, start_ts: u64) -> BatchResult<()> {
        self.transport.check_visibility(start_ts)
    }

    /// 返回可用 MPP store 数量（带 TTL 缓存）。
    pub fn get_mpp_store_count(&self) -> BatchResult<usize> {
        self.store_count
            .get(MPP_STORE_COUNT_TTL_MICROS, || self.transport.all_stores())
    }
}

/// Go 风格别名：MPPClient。
pub type MPPClient = MppClient;
#[allow(non_camel_case_types)]
/// Go 风格别名：mppStoreCnt。
pub type mppStoreCnt = MppStoreCount;

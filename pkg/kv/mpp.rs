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

// MPP（Massively Parallel Processing，大规模并行处理）任务与客户端抽象。
//
// MPP 将查询片段下推到 TiFlash 等计算节点并行执行。本模块定义：
// - MPP 协议版本（`MppVersion`）解析与协商；
// - `MPPTask` / `MPPDispatchRequest` 等任务元数据与下发请求；
// - `MPPClient`：构造任务、下发、建连、取消；
// - `MppCoordinator`：协调端执行与状态上报。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    Backoffer, Context, Error, KeyRange, MPPStreamResponse, PartitionIDAndRanges, Response,
    ResultSubset, tiflash, tiflashcompute,
};

pub use kvproto::mpp::{DispatchTaskResponse, ReportTaskStatusRequest, TaskMeta};

/// MPP 协议版本号封装；用于 TiDB 与 TiFlash 之间的能力协商。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MppVersion(pub i64);

impl Default for MppVersion {
    fn default() -> Self {
        MppVersionV0
    }
}

/// MPP 协议版本 0。
pub const MppVersionV0: MppVersion = MppVersion(0);
/// MPP 协议版本 1。
pub const MppVersionV1: MppVersion = MppVersion(1);
/// MPP 协议版本 2。
pub const MppVersionV2: MppVersion = MppVersion(2);
/// MPP 协议版本 3。
pub const MppVersionV3: MppVersion = MppVersion(3);
/// 内部上限哨兵（不对外暴露为合法 newest）。
const mppVersionMax: MppVersion = MppVersion(4);
/// 当前实现支持的最新合法版本（max - 1）。
const newestMppVersion: MppVersion = MppVersion(mppVersionMax.0 - 1);
/// 未指定版本（协商时表示由对端决定）。
pub const MppVersionUnspecified: MppVersion = MppVersion(-1);
/// 未指定版本的字符串名。
pub const MppVersionUnspecifiedName: &str = "UNSPECIFIED";

impl MppVersion {
    /// 返回底层 i64 版本号。
    pub fn ToInt64(self) -> i64 {
        self.0
    }
}

/// 将版本名或数字字符串解析为 `MppVersion`；第二个返回值表示是否解析成功。
pub fn ToMppVersion(name: &str) -> (MppVersion, bool) {
    let upper = name.to_uppercase();
    if upper == MppVersionUnspecifiedName {
        return (MppVersionUnspecified, true);
    }
    let Ok(value) = upper.parse::<i64>() else {
        return (MppVersionUnspecified, false);
    };
    let version = MppVersion(value);
    if version >= MppVersionUnspecified && version <= newestMppVersion {
        (version, true)
    } else {
        (MppVersionUnspecified, false)
    }
}

/// 返回当前支持的最新 MPP 协议版本。
pub fn GetNewestMppVersion() -> MppVersion {
    newestMppVersion
}

/// MPP 任务元数据：至少提供执行节点地址，并支持装箱克隆。
pub trait MPPTaskMeta: Send + Sync {
    /// 返回任务所在 TiFlash/Store 地址。
    fn GetAddress(&self) -> String;
    /// 克隆为新的 trait 对象。
    fn CloneBox(&self) -> Box<dyn MPPTaskMeta>;
}

impl Clone for Box<dyn MPPTaskMeta> {
    fn clone(&self) -> Self {
        self.CloneBox()
    }
}

/// 标识一次 MPP 查询的复合 ID（查询时间戳 + 本地查询 ID + Server ID）。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MPPQueryID {
    /// 查询起始时间戳。
    pub QueryTs: u64,
    /// 进程内本地查询 ID。
    pub LocalQueryID: u64,
    /// 发起查询的 Server ID。
    pub ServerID: u64,
}

/// 单个 MPP 任务的运行时描述（含表、分区、会话与协议版本）。
pub struct MPPTask {
    /// 任务所在节点元数据；根任务（ID=-1）可为空。
    pub Meta: Option<Box<dyn MPPTaskMeta>>,
    /// 任务 ID；`-1` 表示根任务。
    pub ID: i64,
    /// 事务/查询起始时间戳（StartTS）。
    pub StartTs: u64,
    /// Gather 阶段 ID。
    pub GatherID: u64,
    /// 所属 MPP 查询 ID。
    pub MppQueryID: MPPQueryID,
    /// 关联表 ID。
    pub TableID: i64,
    /// 使用的 MPP 协议版本。
    pub MppVersion: MppVersion,
    /// 会话连接 ID。
    pub SessionID: u64,
    /// 会话别名。
    pub SessionAlias: String,
    /// 分区表 ID 列表。
    pub PartitionTableIDs: Vec<i64>,
    /// 是否启用 TiFlash 静态剪枝。
    pub TiFlashStaticPrune: bool,
}

impl Default for MPPTask {
    fn default() -> Self {
        Self {
            Meta: None,
            ID: 0,
            StartTs: 0,
            GatherID: 0,
            MppQueryID: MPPQueryID::default(),
            TableID: 0,
            MppVersion: MppVersionV0,
            SessionID: 0,
            SessionAlias: String::new(),
            PartitionTableIDs: Vec::new(),
            TiFlashStaticPrune: false,
        }
    }
}

impl Clone for MPPTask {
    fn clone(&self) -> Self {
        Self {
            Meta: self.Meta.clone(),
            ID: self.ID,
            StartTs: self.StartTs,
            GatherID: self.GatherID,
            MppQueryID: self.MppQueryID,
            TableID: self.TableID,
            MppVersion: self.MppVersion,
            SessionID: self.SessionID,
            SessionAlias: self.SessionAlias.clone(),
            PartitionTableIDs: self.PartitionTableIDs.clone(),
            TiFlashStaticPrune: self.TiFlashStaticPrune,
        }
    }
}

impl MPPTask {
    /// 转换为 protobuf `TaskMeta`；非根任务必须带有节点地址。
    pub fn ToPB(&self) -> TaskMeta {
        let mut meta = TaskMeta {
            start_ts: self.StartTs,
            gather_id: self.GatherID,
            query_ts: self.MppQueryID.QueryTs,
            local_query_id: self.MppQueryID.LocalQueryID,
            server_id: self.MppQueryID.ServerID,
            task_id: self.ID,
            mpp_version: self.MppVersion.ToInt64(),
            connection_id: self.SessionID,
            connection_alias: self.SessionAlias.clone(),
            ..Default::default()
        };
        // 根任务 ID 为 -1，不下发地址；其它任务从 Meta 取 Store 地址。
        if self.ID != -1 {
            meta.address = self
                .Meta
                .as_ref()
                .expect("MPPTask.Meta is required for non-root tasks")
                .GetAddress();
        }
        meta
    }
}

/// MPP 任务生命周期状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MppTaskStates {
    /// 已就绪，等待调度。
    MppTaskReady = 0,
    /// 正在运行。
    MppTaskRunning = 1,
    /// 已取消。
    MppTaskCancelled = 2,
    /// 已完成。
    MppTaskDone = 3,
}

/// 向 TiFlash 下发单个 MPP 任务的请求载荷。
pub struct MPPDispatchRequest {
    /// 序列化后的执行计划 / DAG 数据。
    pub Data: Vec<u8>,
    /// 目标节点元数据。
    pub Meta: Option<Box<dyn MPPTaskMeta>>,
    /// 是否为根任务。
    pub IsRoot: bool,
    /// 超时（秒或约定单位，与 Go 保持一致）。
    pub Timeout: u64,
    /// Schema 版本。
    pub SchemaVar: i64,
    /// 起始时间戳。
    pub StartTs: u64,
    /// 查询 ID。
    pub MppQueryID: MPPQueryID,
    /// Gather ID。
    pub GatherID: u64,
    /// 任务 ID。
    pub ID: i64,
    /// 协议版本。
    pub MppVersion: MppVersion,
    /// 协调者（Coordinator）地址。
    pub CoordinatorAddress: String,
    /// 是否上报执行摘要。
    pub ReportExecutionSummary: bool,
    /// 当前任务状态。
    pub State: MppTaskStates,
    /// 资源组名称。
    pub ResourceGroupName: String,
    /// 连接 ID。
    pub ConnectionID: u64,
    /// 连接别名。
    pub ConnectionAlias: String,
    /// SQL digest（语句摘要）。
    pub SQLDigest: String,
    /// Plan digest（计划摘要）。
    pub PlanDigest: String,
}

impl Clone for MPPDispatchRequest {
    fn clone(&self) -> Self {
        Self {
            Data: self.Data.clone(),
            Meta: self.Meta.clone(),
            IsRoot: self.IsRoot,
            Timeout: self.Timeout,
            SchemaVar: self.SchemaVar,
            StartTs: self.StartTs,
            MppQueryID: self.MppQueryID,
            GatherID: self.GatherID,
            ID: self.ID,
            MppVersion: self.MppVersion,
            CoordinatorAddress: self.CoordinatorAddress.clone(),
            ReportExecutionSummary: self.ReportExecutionSummary,
            State: self.State,
            ResourceGroupName: self.ResourceGroupName.clone(),
            ConnectionID: self.ConnectionID,
            ConnectionAlias: self.ConnectionAlias.clone(),
            SQLDigest: self.SQLDigest.clone(),
            PlanDigest: self.PlanDigest.clone(),
        }
    }
}

impl Default for MPPDispatchRequest {
    fn default() -> Self {
        Self {
            Data: Vec::new(),
            Meta: None,
            IsRoot: false,
            Timeout: 0,
            SchemaVar: 0,
            StartTs: 0,
            MppQueryID: MPPQueryID::default(),
            GatherID: 0,
            ID: 0,
            MppVersion: MppVersionV0,
            CoordinatorAddress: String::new(),
            ReportExecutionSummary: false,
            State: MppTaskStates::MppTaskReady,
            ResourceGroupName: String::new(),
            ConnectionID: 0,
            ConnectionAlias: String::new(),
            SQLDigest: String::new(),
            PlanDigest: String::new(),
        }
    }
}

/// 取消 MPP 任务时传入的 Store 地址集合与相关请求列表。
pub struct CancelMPPTasksParam {
    /// 需要取消的 Store 地址集合（值为占位 bool）。
    pub StoreAddr: HashMap<String, bool>,
    /// 关联的下发请求。
    pub Reqs: Vec<MPPDispatchRequest>,
}

/// 建立 MPP 数据连接所需的参数。
pub struct EstablishMPPConnsParam<'a> {
    pub Ctx: &'a Context,
    pub Req: &'a MPPDispatchRequest,
    pub TaskMeta: &'a TaskMeta,
    /// 退避器（Backoffer）：RPC 失败时按策略重试。
    pub Bo: &'a mut Backoffer,
}

/// 下发单个 MPP 任务所需的参数。
pub struct DispatchMPPTaskParam<'a> {
    pub Ctx: &'a Context,
    pub Req: &'a MPPDispatchRequest,
    /// 是否收集执行信息。
    pub EnableCollectExecutionInfo: bool,
    pub Bo: &'a mut Backoffer,
}

/// MPP 客户端：负责任务构造、下发、建连、取消与可见性检查。
pub trait MPPClient: Send + Sync {
    /// 根据 key range / 分区范围在集群上构造 MPP 任务元数据列表。
    fn ConstructMPPTasks(
        &self,
        ctx: &Context,
        req: &MPPBuildTasksRequest,
        timeout: Duration,
        policy: tiflashcompute::DispatchPolicy,
        replica_read: tiflash::ReplicaRead,
        on_error: &mut dyn FnMut(Error),
    ) -> Result<Vec<Box<dyn MPPTaskMeta>>, Error>;

    /// 下发单个 MPP 任务，返回 protobuf 响应及是否需要重试等标志。
    fn DispatchMPPTask(
        &self,
        param: DispatchMPPTaskParam<'_>,
    ) -> Result<(DispatchTaskResponse, bool), Error>;
    /// 建立到任务节点的流式数据连接。
    fn EstablishMPPConns(
        &self,
        param: EstablishMPPConnsParam<'_>,
    ) -> Result<(MPPStreamResponse, bool), Error>;
    /// 取消已下发的 MPP 任务。
    fn CancelMPPTasks(&self, param: CancelMPPTasksParam);
    /// 检查给定 start_time 对应的快照对 MPP 是否仍可见。
    fn CheckVisibility(&self, start_time: u64) -> Result<(), Error>;
    /// 返回可用于 MPP 的 Store 数量。
    fn GetMPPStoreCount(&self) -> Result<i32, Error>;
}

/// 状态上报请求包装。
pub struct ReportStatusRequest {
    pub Request: ReportTaskStatusRequest,
}

/// 接收 MPP 任务状态上报的接口。
pub trait MppStatusReporter: Send + Sync {
    fn ReportStatus(&self, info: ReportStatusRequest) -> Result<(), Error>;
}

/// MPP 协调者：同时作为响应流，负责调度执行与节点统计。
pub trait MppCoordinator: Response + Send {
    /// Starts dispatch. The coordinator itself is the returned response stream
    /// in Go, so Rust callers retain `&mut self` and receive only key ranges.
    ///
    /// 启动下发；在 Go 中协调者本身即响应流，Rust 侧保留 `&mut self` 并只返回 key ranges。
    fn Execute(&mut self, ctx: &Context) -> Result<Vec<KeyRange>, Error>;
    fn ReportStatus(&mut self, info: ReportStatusRequest) -> Result<(), Error>;
    fn StatusReporter(&self) -> Arc<dyn MppStatusReporter>;
    fn IsClosed(&self) -> bool;
    /// 参与本次 MPP 查询的节点数。
    fn GetNodeCnt(&self) -> i32;
}

/// 构造 MPP 任务时传入的 key range / 分区范围与 StartTS。
pub struct MPPBuildTasksRequest {
    /// 非分区表的 key range 列表；与 `PartitionIDAndRanges` 二选一使用。
    pub KeyRanges: Option<Vec<KeyRange>>,
    /// 查询起始时间戳。
    pub StartTS: u64,
    /// 分区表：每个分区 ID 对应一组 key range。
    pub PartitionIDAndRanges: Vec<PartitionIDAndRanges>,
}

impl MPPBuildTasksRequest {
    /// 将 key range / 分区范围序列化为缓存键字符串（与 Go 拼接格式一致）。
    pub fn ToString(&self) -> String {
        let mut output = String::new();
        if let Some(ranges) = &self.KeyRanges {
            for (index, key_range) in ranges.iter().enumerate() {
                output.push_str(&format!("range_id{index}"));
                output.push_str(&key_range.StartKey.String());
                output.push_str(&key_range.EndKey.String());
            }
            return output;
        }

        for partition in &self.PartitionIDAndRanges {
            output.push_str(&format!("partition_id{}", partition.ID));
            for (index, key_range) in partition.KeyRanges.iter().enumerate() {
                output.push_str(&format!("range_id{index}"));
                output.push_str(&key_range.StartKey.String());
                output.push_str(&key_range.EndKey.String());
            }
        }
        output
    }
}

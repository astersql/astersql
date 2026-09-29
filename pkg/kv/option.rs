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

// 事务选项（Transaction Option）标识与请求来源辅助类型。
//
// 事务选项 ID 是各 `Transaction` 实现共享的 ABI：通过 `SetOption`/`GetOption`
// 按整数键挂载异构配置（隔离级别、副本读策略、资源组、RPC 拦截器等）。
// 本模块还定义内部事务请求来源字符串常量，以及 TxnSource 位图中
// TiCDC / 有损 DDL 重组来源字段的编解码函数。

use crate::{Context, Error, errors};

// Transaction option IDs are an ABI shared by Transaction implementations.
/// 事务选项：binlog 相关信息。
pub const BinlogInfo: i32 = 1;
/// 事务选项：schema 变更检查器。
pub const SchemaChecker: i32 = 2;
/// 事务选项：隔离级别（如 SI / RC）。
pub const IsolationLevel: i32 = 3;
/// 事务选项：请求优先级。
pub const Priority: i32 = 4;
/// 事务选项：读时不填充 block cache。
pub const NotFillCache: i32 = 5;
/// 事务选项：同步刷盘相关。
pub const SyncLog: i32 = 6;
/// 事务选项：仅返回 key（扫描时省略 value）。
pub const KeyOnly: i32 = 7;
/// 事务选项：悲观事务（Pessimistic Transaction）标记。
pub const Pessimistic: i32 = 8;
/// 事务选项：快照时间戳（Snapshot TS）。
pub const SnapshotTS: i32 = 9;
/// 事务选项：副本读（Replica Read）策略。
pub const ReplicaRead: i32 = 10;
/// 事务选项：任务 ID。
pub const TaskID: i32 = 11;
/// 事务选项：InfoSchema 句柄。
pub const InfoSchema: i32 = 12;
/// 事务选项：是否收集运行时统计。
pub const CollectRuntimeStats: i32 = 13;
/// 事务选项：schema amender。
pub const SchemaAmender: i32 = 14;
/// 事务选项：采样步长。
pub const SampleStep: i32 = 15;
/// 事务选项：提交钩子（Commit Hook）。
pub const CommitHook: i32 = 16;
/// 事务选项：启用 Async Commit（异步提交）。
pub const EnableAsyncCommit: i32 = 17;
/// 事务选项：启用 1PC（单阶段提交）。
pub const Enable1PC: i32 = 18;
/// 事务选项：保证线性一致性。
pub const GuaranteeLinearizability: i32 = 19;
/// 事务选项：事务作用域（TxnScope）。
pub const TxnScope: i32 = 20;
/// 事务选项：读副本作用域。
pub const ReadReplicaScope: i32 = 21;
/// 事务选项：是否为 stale 只读。
pub const IsStalenessReadOnly: i32 = 22;
/// 事务选项：按 Store Label 匹配。
pub const MatchStoreLabels: i32 = 23;
/// 事务选项：资源组标签。
pub const ResourceGroupTag: i32 = 24;
/// 事务选项：资源组标签生成器。
pub const ResourceGroupTagger: i32 = 25;
/// 事务选项：KV 过滤器。
pub const KVFilter: i32 = 26;
/// 事务选项：快照拦截器。
pub const SnapInterceptor: i32 = 27;
/// 事务选项：提交时间戳上界检查。
pub const CommitTSUpperBoundCheck: i32 = 28;
/// 事务选项：RPC 拦截器。
pub const RPCInterceptor: i32 = 29;
/// 事务选项：表到列的映射。
pub const TableToColumnMaps: i32 = 30;
/// 事务选项：断言级别（Assertion Level）。
pub const AssertionLevel: i32 = 31;
/// 事务选项：是否为内部请求来源。
pub const RequestSourceInternal: i32 = 32;
/// 事务选项：请求来源类型字符串。
pub const RequestSourceType: i32 = 33;
/// 事务选项：显式请求来源类型（任务名等）。
pub const ExplicitRequestSourceType: i32 = 34;
/// 事务选项：副本读调整器。
pub const ReplicaReadAdjuster: i32 = 35;
/// 事务选项：扫描批大小。
pub const ScanBatchSize: i32 = 36;
/// 事务选项：事务来源位图（TxnSource）。
pub const TxnSource: i32 = 37;
/// 事务选项：资源组名称。
pub const ResourceGroupName: i32 = 38;
/// 事务选项：基于负载的副本读阈值。
pub const LoadBasedReplicaReadThreshold: i32 = 39;
/// 事务选项：TiKV 客户端读超时。
pub const TiKVClientReadTimeout: i32 = 40;
/// 事务选项：事务大小限制。
pub const SizeLimits: i32 = 41;
/// 事务选项：会话 ID。
pub const SessionID: i32 = 42;
/// 事务选项：后台协程生命周期钩子。
pub const BackgroundGoroutineLifecycleHooks: i32 = 43;
/// 事务选项：prewrite 遇到锁时的策略。
pub const PrewriteEncounterLockPolicy: i32 = 44;

/// 单条目与整事务的大小上限（字节）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxnSizeLimits {
    /// 单条 mutation 的大小上限。
    pub Entry: u64,
    /// 事务总大小上限。
    pub Total: u64,
}

/// 副本读类型：决定读请求发往 Leader / Follower / Learner 等角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ReplicaReadType {
    /// 仅读 Leader。
    ReplicaReadLeader = 0,
    /// 读 Follower。
    ReplicaReadFollower = 1,
    /// Leader 与 Follower 混合。
    ReplicaReadMixed = 2,
    /// 就近读（Closest）。
    ReplicaReadClosest = 3,
    /// 自适应就近读。
    ReplicaReadClosestAdaptive = 4,
    /// 读 Learner。
    ReplicaReadLearner = 5,
    /// 优先 Leader，必要时回退。
    ReplicaReadPreferLeader = 6,
}

impl ReplicaReadType {
    /// 是否属于非纯 Leader 读（可打到 Follower 等）。
    pub fn IsFollowerRead(self) -> bool {
        self != Self::ReplicaReadLeader
    }

    /// 是否为精确的 Closest 读策略。
    pub fn IsClosestRead(self) -> bool {
        self == Self::ReplicaReadClosest
    }
}

/// Context 中挂载 `RequestSource` 所用的键类型占位。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RequestSourceKeyType;

/// 全局请求来源键单例。
pub static RequestSourceKey: RequestSourceKeyType = RequestSourceKeyType;

/// 请求来源：标记内部/外部及具体来源类型字符串。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestSource {
    /// 是否为内部事务请求。
    pub RequestSourceInternal: bool,
    /// 来源类型（如 `ddl`、`stats`）。
    pub RequestSourceType: String,
    /// 显式任务名等更细粒度来源。
    pub ExplicitRequestSourceType: String,
}

/// 在 Context 上设置内部请求来源类型。
pub fn WithInternalSourceType(ctx: Context, source: impl Into<String>) -> Context {
    ctx.with_request_source(RequestSource {
        RequestSourceInternal: true,
        RequestSourceType: source.into(),
        ExplicitRequestSourceType: String::new(),
    })
}

/// 在 Context 上同时设置内部来源类型与显式任务名。
pub fn WithInternalSourceAndTaskType(
    ctx: Context,
    source: impl Into<String>,
    task_name: impl Into<String>,
) -> Context {
    ctx.with_request_source(RequestSource {
        RequestSourceInternal: true,
        RequestSourceType: source.into(),
        ExplicitRequestSourceType: task_name.into(),
    })
}

/// 从 Context 取出内部请求来源类型字符串；缺失时返回空串。
pub fn GetInternalSourceType(ctx: &Context) -> String {
    ctx.RequestSource()
        .map(|source| source.RequestSourceType.clone())
        .unwrap_or_default()
}

/// 内部事务来源：其它/未分类。
pub const InternalTxnOthers: &str = "others";
/// 内部事务来源：GC。
pub const InternalTxnGC: &str = "gc";
/// 内部事务来源：bootstrap（归入 others）。
pub const InternalTxnBootstrap: &str = InternalTxnOthers;
/// 内部事务来源：元数据（归入 others）。
pub const InternalTxnMeta: &str = InternalTxnOthers;
/// 内部事务来源：DDL。
pub const InternalTxnDDL: &str = "ddl";
/// Materialized view maintenance operations.
pub const InternalTxnMViewMaintenance: &str = "mview_maintain";
/// 内部事务来源：DDL backfill 前缀。
pub const InternalTxnBackfillDDLPrefix: &str = "ddl_";
/// 内部事务来源：缓存表（归入 others）。
pub const InternalTxnCacheTable: &str = InternalTxnOthers;
/// 内部事务来源：统计信息。
pub const InternalTxnStats: &str = "stats";
/// 内部事务来源：前台优先统计任务。
pub const InternalTxnStatsForegroundPriority: &str = "StatsForegroundPriority";
/// 内部事务来源：绑定信息（归入 others）。
pub const InternalTxnBindInfo: &str = InternalTxnOthers;
/// 内部事务来源：工作负载学习。
pub const InternalTxnWorkloadLearning: &str = "WorkloadLearning";
/// 内部事务来源：系统变量（归入 others）。
pub const InternalTxnSysVar: &str = InternalTxnOthers;
/// 内部事务来源：遥测（归入 others）。
pub const InternalTxnTelemetry: &str = InternalTxnOthers;
/// 内部事务来源：管理命令。
pub const InternalTxnAdmin: &str = "admin";
/// 内部事务来源：权限（归入 others）。
pub const InternalTxnPrivilege: &str = InternalTxnOthers;
/// 内部事务来源：工具类。
pub const InternalTxnTools: &str = "tools";
/// 内部事务来源：BR 备份恢复。
pub const InternalTxnBR: &str = "br";
/// 内部事务来源：Lightning 导入。
pub const InternalTxnLightning: &str = "lightning";
/// 内部事务来源：Trace。
pub const InternalTxnTrace: &str = "Trace";
/// 内部事务来源：TTL。
pub const InternalTxnTTL: &str = "TTL";
/// 内部事务来源：LOAD DATA。
pub const InternalLoadData: &str = "LoadData";
/// 内部事务来源：IMPORT INTO。
pub const InternalImportInto: &str = "ImportInto";
/// 内部事务来源：分布式任务。
pub const InternalDistTask: &str = "DistTask";
/// 内部事务来源：定时器。
pub const InternalTimer: &str = "Timer";
/// 内部事务来源：DDL Notifier。
pub const InternalDDLNotifier: &str = "DDLNotifier";

/// TiCDC write source 占用的低位比特数。
const cdcWriteSourceBits: u64 = 8;
/// TiCDC write source 位掩码上限值。
const cdcWriteSourceMax: u64 = (1 << cdcWriteSourceBits) - 1;
/// 有损 DDL 重组来源占用的比特数。
const lossyDDLReorgSourceBits: u64 = 8;
/// 列重组场景下的有损 DDL 来源取值。
pub const LossyDDLColumnReorgSource: u64 = 1;
/// 有损 DDL 重组来源最大值。
const lossyDDLReorgSourceMax: u64 = (1 << lossyDDLReorgSourceBits) - 1;
/// 有损 DDL 重组来源在 TxnSource 中的位移（紧接 CDC 字段之后）。
const lossyDDLReorgSourceShift: u64 = cdcWriteSourceBits;
/// Lightning 物理导入占用的 TxnSource 标志位。
pub const LightningPhysicalImportTxnSource: u64 = 1 << 16;

/// 将 TiCDC 写来源写入 TxnSource 低位；越界时返回错误。
pub fn SetCDCWriteSource(txn_source: &mut u64, value: u64) -> Result<(), Error> {
    // Keep the Go implementation's effective upper-bound check (8), including
    // its historically broader error-message wording.
    if value > cdcWriteSourceBits {
        return Err(errors::New(format!(
            "value {value} is out of TiCDC write source range, should be in [1, 15]"
        )));
    }
    *txn_source |= value;
    Ok(())
}

/// 提取 TxnSource 中的 CDC write source 字段。
fn getCDCWriteSource(txn_source: u64) -> u64 {
    txn_source & cdcWriteSourceMax
}

/// 判断 CDC write source 是否已设置。
fn isCDCWriteSourceSet(txn_source: u64) -> bool {
    txn_source & cdcWriteSourceMax != 0
}

/// 公开接口：读取 CDC write source。
pub fn GetCDCWriteSource(txn_source: u64) -> u64 {
    getCDCWriteSource(txn_source)
}

/// 公开接口：判断 CDC write source 是否已设置。
pub fn IsCDCWriteSourceSet(txn_source: u64) -> bool {
    isCDCWriteSourceSet(txn_source)
}

/// 将有损 DDL 重组来源写入 TxnSource 高位字段；越界时返回错误。
pub fn SetLossyDDLReorgSource(txn_source: &mut u64, value: u64) -> Result<(), Error> {
    if value > lossyDDLReorgSourceMax {
        return Err(errors::New(format!(
            "value {value} is out of lossy DDL reorg source range, should be in [1, {lossyDDLReorgSourceMax}]"
        )));
    }
    *txn_source |= value << lossyDDLReorgSourceShift;
    Ok(())
}

/// 提取有损 DDL 重组来源字段。
fn getLossyDDLReorgSource(txn_source: u64) -> u64 {
    (txn_source >> lossyDDLReorgSourceShift) & lossyDDLReorgSourceMax
}

/// 判断有损 DDL 重组来源是否已设置。
fn isLossyDDLReorgSourceSet(txn_source: u64) -> bool {
    txn_source >> lossyDDLReorgSourceShift != 0
}

/// 公开接口：读取有损 DDL 重组来源。
pub fn GetLossyDDLReorgSource(txn_source: u64) -> u64 {
    getLossyDDLReorgSource(txn_source)
}

/// 公开接口：判断有损 DDL 重组来源是否已设置。
pub fn IsLossyDDLReorgSourceSet(txn_source: u64) -> bool {
    isLossyDDLReorgSourceSet(txn_source)
}

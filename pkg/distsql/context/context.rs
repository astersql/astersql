// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DistSQL 执行上下文模块。
//
// DistSQL（分布式 SQL）是 TiDB/AsterSQL 中负责把逻辑查询下推到底层存储层
// （TiKV/TiFlash）执行的组件层。本模块定义 [`DistSQLContext`]，它汇聚了向
// Coprocessor（协处理器，即存储节点上执行下推计算的模块）发起请求时所需的
// 全部信息：告警收集、KV 客户端、读一致性级别、TiFlash 相关参数、资源组、
// 分页（paging）、运行时统计等。
//
// 该结构体是 Go 版 TiDB `distsql.DistSQLContext` 的机械迁移：Go 中的指针与
// 接口字段在 Rust 中用引用或 `Arc`（原子引用计数共享指针）表示，从而保留
// 「浅拷贝」语义，同时让 `Detach`（从会话上下文分离）时需要独立重建的少数
// 字段（CPU 用量、KV 变量、已读键计数器）能够单独处理。

// 允许非 snake_case 命名：字段名沿用 Go 源码的驼峰命名以便逐字段对照迁移。
#![allow(non_snake_case)]

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crate::{
    contextutil, errctx, errors, execdetails, kv, memory, mysql, ppcpuusage, sqlkiller, tiflash,
    tikvstore,
};

/// Shared warning sink corresponding to Go's `contextutil.WarnAppender` interface value.
///
/// 共享的告警接收器，对应 Go 中 `contextutil.WarnAppender` 接口值。
/// 执行过程中产生的告警（如隐式类型转换、超时提示等）会被追加到该接收器，
/// 最终汇总返回给客户端。使用 `Arc<dyn ...>` 以便在多个上下文之间共享同一个实例。
pub type WarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>;

/// Context values whose concrete interface methods are owned by downstream packages.
///
/// `DistSQLContext` only retains and forwards these Go interface values. Type erasure keeps
/// their shared-pointer identity without inventing a second implementation in this package.
///
/// 具体接口方法由下游包持有的上下文值。
/// `DistSQLContext` 只是持有并向下传递这些 Go 接口值，本包并不关心其具体类型。
/// 这里用 `Arc<dyn Any>`（类型擦除，即抹去具体类型只保留通用引用）来保持它们
/// 共享指针的身份，而无需在本包重复实现一份逻辑。
pub type SharedContextValue = Arc<dyn Any + Send + Sync>;

/// DistSQLContext provides all information needed by functions in `distsql`.
///
/// Go pointer and interface fields are represented by references or `Arc`. This makes the
/// shallow-copy part of `Detach` explicit while allowing the three Go exceptions (CPU usage,
/// KV variables, and max-keys-read counter) to be rebuilt independently.
///
/// DistSQLContext 提供 `distsql` 中各函数所需的全部信息。
/// Go 中的指针与接口字段用引用或 `Arc` 表示。这样既让 `Detach` 的浅拷贝部分
/// 显式可见，又允许三个 Go 版本的例外字段（CPU 用量、KV 变量、已读键上限计数器）
/// 被独立重建。
/// 生命周期参数 `'a` 用于借用会话级的 `SQLKiller` 与 KV 变量中的中止信号。
pub struct DistSQLContext<'a> {
    /// 告警处理器：执行期间产生的告警统一追加到此处。
    pub WarnHandler: WarnAppenderRef,

    /// 是否处于内部受限 SQL（如系统统计、后台任务发起的 SQL）执行中。
    pub InRestrictedSQL: bool,
    /// 与存储层通信的 KV 客户端；用于发送 Coprocessor 请求。
    pub Client: Option<Arc<dyn kv::Client + Send + Sync>>,

    /// 是否启用基于速率限制的内存控制动作。
    pub EnabledRateLimitAction: bool,
    /// 是否启用 Chunk 格式的 RPC 传输（按列式 Chunk 返回结果，减少序列化开销）。
    pub EnableChunkRPC: bool,
    /// 原始 SQL 文本，主要用于日志与诊断。
    pub OriginalSQL: String,
    /// 传递给 TiKV 客户端的变量（退避策略、中止信号等）。
    pub KVVars: Option<tikvstore::Variables<'a>>,
    /// KV 执行计数器，具体类型由下游包持有（类型擦除）。
    pub KvExecCounter: Option<SharedContextValue>,
    /// RU（Request Unit，资源计量单位）V2 版指标收集器。
    pub RUV2Metrics: Option<Arc<execdetails::RUV2Metrics>>,
    /// 会话级内存追踪器，用于统计并限制本会话的内存使用。
    pub SessionMemTracker: Option<Arc<memory::Tracker>>,

    /// 会话时区，用于时间类型的下推计算与结果解释。
    pub Location: Option<Arc<chrono_tz::Tz>>,
    /// 运行时统计收集器，汇总各算子的执行耗时与行数等信息。
    pub RuntimeStatsColl: Option<Arc<execdetails::RuntimeStatsColl>>,
    /// SQL 终止器：当查询被 KILL 或超时时用于中止执行（借用会话所有的实例）。
    pub SQLKiller: Option<&'a sqlkiller::SQLKiller>,
    /// SQL 级 CPU 用量统计。
    pub CPUUsage: Option<Arc<ppcpuusage::SQLCPUUsages>>,
    /// 错误上下文：控制哪些错误降级为告警、如何处理截断等。
    pub ErrCtx: errctx::Context,

    // TiFlash related configurations.
    // TiFlash 相关配置。TiFlash 是列存引擎，主要服务 OLAP（分析型）查询。
    /// TiFlash 副本读策略。
    pub TiFlashReplicaRead: tiflash::ReplicaRead,
    /// TiFlash 单查询最大线程数。
    pub TiFlashMaxThreads: i64,
    /// TiFlash 中 Join 落盘（spill）到外部存储前的最大内存字节数。
    pub TiFlashMaxBytesBeforeExternalJoin: i64,
    /// TiFlash 中 GROUP BY 落盘前的最大内存字节数。
    pub TiFlashMaxBytesBeforeExternalGroupBy: i64,
    /// TiFlash 中排序落盘前的最大内存字节数。
    pub TiFlashMaxBytesBeforeExternalSort: i64,
    /// TiFlash 单节点单查询的最大内存用量。
    pub TiFlashMaxQueryMemoryPerNode: i64,
    /// TiFlash 触发落盘的内存占比阈值。
    pub TiFlashQuerySpillRatio: f64,
    /// TiFlash 使用的 Hash Join 算法版本。
    pub TiFlashHashJoinVersion: String,

    /// DistSQL 并发度：同时向存储层发起的请求并发数。
    pub DistSQLConcurrency: isize,
    /// 副本读类型：从 Leader、Follower 还是就近副本读取。
    pub ReplicaReadType: kv::ReplicaReadType,
    /// 是否使用弱一致性读（允许读到略旧数据以提升性能）。
    pub WeakConsistency: bool,
    /// 是否启用 RC（读已提交）隔离级别下的时间戳检查优化。
    pub RCCheckTS: bool,
    /// 是否跳过填充存储层的块缓存（避免大范围扫描污染缓存）。
    pub NotFillCache: bool,
    /// 任务 ID，用于关联同一语句下的多个子请求。
    pub TaskID: u64,
    /// 请求优先级（高/普通/低）。
    pub Priority: mysql::PriorityEnum,
    /// 资源组标签构造器：为请求打上资源组标签以便计量与限流。
    pub ResourceGroupTagger: Option<Arc<kv::ResourceGroupTagBuilder>>,
    /// 是否启用分页（paging）：将大范围扫描拆成多个小页逐步返回。
    pub EnablePaging: bool,
    /// 分页最小页大小（行数）。
    pub MinPagingSize: isize,
    /// 分页最大页大小（行数）。
    pub MaxPagingSize: isize,
    /// 分页按字节计的目标大小。
    pub PagingSizeBytes: isize,
    /// 请求来源类型（内部推断），用于计量与诊断。
    pub RequestSourceType: String,
    /// 显式指定的请求来源类型，优先于推断值。
    pub ExplicitRequestSourceType: String,
    /// Store 批量请求大小：将多个 key 的请求合并批量发送。
    pub StoreBatchSize: isize,
    /// 资源组名称，用于资源隔离与配额管理。
    pub ResourceGroupName: String,
    /// 基于负载的就近副本读阈值：当 Leader 负载过高时改读其他副本。
    pub LoadBasedReplicaReadThreshold: Duration,
    /// Runaway（失控查询）检查器，具体实现由下游包持有。
    pub RunawayChecker: Option<SharedContextValue>,
    /// RU 消耗上报器，具体实现由下游包持有。
    pub RUConsumptionReporter: Option<SharedContextValue>,
    /// TiKV 客户端读超时（毫秒）。
    pub TiKVClientReadTimeout: u64,
    /// 语句最大执行时间。
    pub MaxExecutionTime: u64,
    /// 允许读取的最大键数量。
    pub MaxKeysRead: u64,
    /// Statement-wide max-keys-read accumulator. `Detach` creates a fresh zero value.
    /// 语句级已读键累加器。`Detach` 会创建一个全新的零值计数器。
    pub MaxKeysReadCounter: Option<Arc<AtomicU64>>,

    /// 就近副本读的距离阈值。
    pub ReplicaClosestReadThreshold: i64,
    /// 连接 ID。
    pub ConnectionID: u64,
    /// 会话别名，便于在日志中识别会话。
    pub SessionAlias: String,

    /// 执行细节的线程安全汇总（各类耗时、扫描量等）。
    pub ExecDetails: Option<Arc<execdetails::SyncExecDetails>>,

    /// Only one cop-reader can use the lite worker at a time.
    /// 同一时刻只允许一个 cop-reader（Coprocessor 读取器）使用轻量 worker。
    /// 用原子变量做互斥标志（0 空闲、非 0 占用）。
    pub TryCopLiteWorker: AtomicU32,
}

impl Clone for DistSQLContext<'_> {
    /// 手动实现 `Clone`：大多数字段直接克隆（`Arc` 仅增加引用计数，属于浅拷贝），
    /// 但 `KVVars` 与 `TryCopLiteWorker` 需要特殊处理——原子计数器不能直接派生克隆，
    /// 需读取当前值再新建一个原子变量。
    fn clone(&self) -> Self {
        Self {
            WarnHandler: Arc::clone(&self.WarnHandler),
            InRestrictedSQL: self.InRestrictedSQL,
            Client: self.Client.clone(),
            EnabledRateLimitAction: self.EnabledRateLimitAction,
            EnableChunkRPC: self.EnableChunkRPC,
            OriginalSQL: self.OriginalSQL.clone(),
            KVVars: self.KVVars.as_ref().map(|variables| tikvstore::Variables {
                BackoffLockFast: variables.BackoffLockFast,
                BackOffWeight: variables.BackOffWeight,
                Killed: variables.Killed,
            }),
            KvExecCounter: self.KvExecCounter.clone(),
            RUV2Metrics: self.RUV2Metrics.clone(),
            SessionMemTracker: self.SessionMemTracker.clone(),
            Location: self.Location.clone(),
            RuntimeStatsColl: self.RuntimeStatsColl.clone(),
            SQLKiller: self.SQLKiller,
            CPUUsage: self.CPUUsage.clone(),
            ErrCtx: self.ErrCtx.clone(),
            TiFlashReplicaRead: self.TiFlashReplicaRead,
            TiFlashMaxThreads: self.TiFlashMaxThreads,
            TiFlashMaxBytesBeforeExternalJoin: self.TiFlashMaxBytesBeforeExternalJoin,
            TiFlashMaxBytesBeforeExternalGroupBy: self.TiFlashMaxBytesBeforeExternalGroupBy,
            TiFlashMaxBytesBeforeExternalSort: self.TiFlashMaxBytesBeforeExternalSort,
            TiFlashMaxQueryMemoryPerNode: self.TiFlashMaxQueryMemoryPerNode,
            TiFlashQuerySpillRatio: self.TiFlashQuerySpillRatio,
            TiFlashHashJoinVersion: self.TiFlashHashJoinVersion.clone(),
            DistSQLConcurrency: self.DistSQLConcurrency,
            ReplicaReadType: self.ReplicaReadType,
            WeakConsistency: self.WeakConsistency,
            RCCheckTS: self.RCCheckTS,
            NotFillCache: self.NotFillCache,
            TaskID: self.TaskID,
            Priority: self.Priority,
            ResourceGroupTagger: self.ResourceGroupTagger.clone(),
            EnablePaging: self.EnablePaging,
            MinPagingSize: self.MinPagingSize,
            MaxPagingSize: self.MaxPagingSize,
            PagingSizeBytes: self.PagingSizeBytes,
            RequestSourceType: self.RequestSourceType.clone(),
            ExplicitRequestSourceType: self.ExplicitRequestSourceType.clone(),
            StoreBatchSize: self.StoreBatchSize,
            ResourceGroupName: self.ResourceGroupName.clone(),
            LoadBasedReplicaReadThreshold: self.LoadBasedReplicaReadThreshold,
            RunawayChecker: self.RunawayChecker.clone(),
            RUConsumptionReporter: self.RUConsumptionReporter.clone(),
            TiKVClientReadTimeout: self.TiKVClientReadTimeout,
            MaxExecutionTime: self.MaxExecutionTime,
            MaxKeysRead: self.MaxKeysRead,
            MaxKeysReadCounter: self.MaxKeysReadCounter.clone(),
            ReplicaClosestReadThreshold: self.ReplicaClosestReadThreshold,
            ConnectionID: self.ConnectionID,
            SessionAlias: self.SessionAlias.clone(),
            ExecDetails: self.ExecDetails.clone(),
            TryCopLiteWorker: AtomicU32::new(self.TryCopLiteWorker.load(Ordering::Relaxed)),
        }
    }
}

impl Default for DistSQLContext<'_> {
    /// 构造默认上下文：告警处理器采用静态实现，错误上下文绑定到同一个告警处理器，
    /// 其余字段取零值/空值。
    fn default() -> Self {
        // 创建一个静态告警处理器，并让错误上下文共享它，保证告警统一汇聚。
        let warn_handler: WarnAppenderRef = Arc::new(contextutil::NewStaticWarnHandler(0));
        let err_context = errctx::NewContext(Arc::clone(&warn_handler));
        Self {
            WarnHandler: warn_handler,
            InRestrictedSQL: false,
            Client: None,
            EnabledRateLimitAction: false,
            EnableChunkRPC: false,
            OriginalSQL: String::new(),
            KVVars: None,
            KvExecCounter: None,
            RUV2Metrics: None,
            SessionMemTracker: None,
            Location: None,
            RuntimeStatsColl: None,
            SQLKiller: None,
            CPUUsage: None,
            ErrCtx: err_context,
            TiFlashReplicaRead: tiflash::ReplicaRead::default(),
            TiFlashMaxThreads: 0,
            TiFlashMaxBytesBeforeExternalJoin: 0,
            TiFlashMaxBytesBeforeExternalGroupBy: 0,
            TiFlashMaxBytesBeforeExternalSort: 0,
            TiFlashMaxQueryMemoryPerNode: 0,
            TiFlashQuerySpillRatio: 0.0,
            TiFlashHashJoinVersion: String::new(),
            DistSQLConcurrency: 0,
            ReplicaReadType: kv::ReplicaReadType::ReplicaReadLeader,
            WeakConsistency: false,
            RCCheckTS: false,
            NotFillCache: false,
            TaskID: 0,
            Priority: mysql::NoPriority,
            ResourceGroupTagger: None,
            EnablePaging: false,
            MinPagingSize: 0,
            MaxPagingSize: 0,
            PagingSizeBytes: 0,
            RequestSourceType: String::new(),
            ExplicitRequestSourceType: String::new(),
            StoreBatchSize: 0,
            ResourceGroupName: String::new(),
            LoadBasedReplicaReadThreshold: Duration::ZERO,
            RunawayChecker: None,
            RUConsumptionReporter: None,
            TiKVClientReadTimeout: 0,
            MaxExecutionTime: 0,
            MaxKeysRead: 0,
            MaxKeysReadCounter: None,
            ReplicaClosestReadThreshold: 0,
            ConnectionID: 0,
            SessionAlias: String::new(),
            ExecDetails: None,
            TryCopLiteWorker: AtomicU32::new(0),
        }
    }
}

impl<'a> DistSQLContext<'a> {
    /// AppendWarning appends the warning to the warning handler.
    /// 向告警处理器追加一条告警。
    pub fn AppendWarning(&self, warn: errors::SharedError) {
        self.WarnHandler.AppendWarning(warn);
    }

    /// Detach detaches this context from the session context.
    ///
    /// Most fields retain Go's shallow-copy behavior. CPU usage and KV variables are copied
    /// into new values. A present max-keys-read counter becomes a fresh zero counter, exactly
    /// like `new(atomic.Uint64)` in Go.
    ///
    /// 将本上下文从会话上下文中分离，得到一个可独立生存的副本。
    /// 典型场景是「游标（cursor）」等需要在语句结束后继续存活的后台执行。
    /// 大多数字段沿用 Go 的浅拷贝语义；CPU 用量与 KV 变量会拷贝成新值；若存在
    /// 已读键计数器，则重置为全新的零值计数器，等价于 Go 中的 `new(atomic.Uint64)`。
    pub fn Detach(&self) -> Box<DistSQLContext<'a>> {
        let mut new_context = self.clone();

        // Go deliberately shares SQLKiller so a kill can still stop the background cursor.
        // Go 有意共享 SQLKiller，这样即便语句已分离，一次 KILL 仍能停止后台游标。
        let sql_killer = self
            .SQLKiller
            .expect("DistSQLContext.Detach requires SQLKiller");
        new_context.SQLKiller = Some(sql_killer);

        // 为分离后的上下文创建独立的 CPU 用量统计，并复制当前累计值作为起点。
        let original_cpu_usage = self
            .CPUUsage
            .as_ref()
            .expect("DistSQLContext.Detach requires CPUUsage");
        let new_cpu_usage = Arc::new(ppcpuusage::SQLCPUUsages::default());
        new_cpu_usage.SetCPUUsages(original_cpu_usage.GetCPUUsages());
        new_context.CPUUsage = Some(new_cpu_usage);

        // 重建 KV 变量：退避配置沿用原值，但中止信号改为指向共享的 SQLKiller，
        // 以保证分离后的执行仍能响应 KILL。
        let original_kv_vars = self
            .KVVars
            .as_ref()
            .expect("DistSQLContext.Detach requires KVVars");
        new_context.KVVars = Some(tikvstore::Variables {
            BackoffLockFast: original_kv_vars.BackoffLockFast,
            BackOffWeight: original_kv_vars.BackOffWeight,
            Killed: &sql_killer.Signal,
        });

        // 若原上下文有已读键计数器，则分离后使用全新的零值计数器，避免共享累加。
        if self.MaxKeysReadCounter.is_some() {
            new_context.MaxKeysReadCounter = Some(Arc::new(AtomicU64::new(0)));
        }

        Box::new(new_context)
    }
}

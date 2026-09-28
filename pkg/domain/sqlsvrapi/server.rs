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

// SQL Server API（sqlsvrapi）核心接口定义。
//
// 抽象 Domain 对 SQL 执行相关能力的访问：keyspace 范围的 KV 存储与系统
// session 池、table-mode DDL 提交，以及跨 keyspace 的运行时句柄获取。
// keyspace：TiDB next-gen 中的逻辑租户/命名空间隔离单元。

use std::error::Error;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::kv::Storage;
use crate::meta::model::AlterTableModeTarget;
use crate::owner::Manager;
use crate::util::DestroyableSessionPool;

/// 带取消语义的上下文，对齐 Go 的 `context.Context`（此处用 CancellationToken）。
/// Cancellation-aware counterpart of Go's `context.Context` for this API.
pub type Context = CancellationToken;

/// SQL server 运行时操作返回的错误类型。
/// Error returned by SQL server runtime operations.
pub type SqlSvrError = Box<dyn Error + Send + Sync + 'static>;

// Runtime is the runtime view for accessing a keyspace through KV/session and
// for submitting table-mode DDL operations to that keyspace.
//
// TODO: Runtime is a historical name and no longer describes this interface
// precisely after table-mode DDL submission was added. Keep the name for now to
// avoid churn until a better keyspace-scoped abstraction is introduced.
//
// Runtime 对应 Go 接口：提供 keyspace 的 KV/session 视图，并暴露 table-mode DDL 提交入口。
/// keyspace 作用域的运行时视图：KV、系统 session 池与 table-mode DDL 提交。
pub trait Runtime: Send + Sync {
    /// 返回该 keyspace 的 KV 存储句柄。
    /// Returns the keyspace-scoped KV storage.
    fn Store(&self) -> Arc<dyn Storage + Send + Sync>;

    /// 返回该 keyspace 的系统 session 池（内部 SQL / 元数据访问用）。
    /// Returns the keyspace-scoped system session pool.
    fn SysSessionPool(&self) -> Arc<dyn DestroyableSessionPool>;

    // AlterTableMode submits an internal table-mode DDL and waits for the result.
    //
    // SchemaID, TableID, and TargetMode are required caller inputs.
    // Cross-keyspace callers must also provide SchemaName and TableName; the
    // implementation validates them against resolved metadata. CurrentMode is
    // resolved by the implementation from current metadata before building the DDL
    // job. The current-keyspace implementation delegates to the local DDL
    // executor, which re-resolves names by ID. ctx is honored by context-aware
    // submit/wait paths, while the local DDL executor path does not currently
    // honor cancellation after the call starts.
    //
    /// 提交内部 table-mode DDL 并等待结果（SchemaID/TableID/TargetMode 必填）。
    fn AlterTableMode(&self, ctx: Context, target: AlterTableModeTarget)
    -> Result<(), SqlSvrError>;
}

// KSRuntimeHandle is an acquired runtime handle for a target keyspace.
// KSRuntimeHandle 嵌入 Runtime，并额外要求 Release 释放已获取的 handle。
/// 已获取的目标 keyspace 运行时句柄；用完后须 `Release`，不拥有 Runtime 生命周期。
pub trait KSRuntimeHandle: Runtime {
    // Release releases the holding of the runtime handle. After calling Release,
    // the handle should not be used anymore.
    // the underlying runtime has different lifecycle, the handle is just a view
    // and does not manage the lifecycle of the runtime.
    //
    /// 释放对运行时句柄的持有；之后不应再使用该句柄。
    fn Release(&self);
}

// Server defines the interface for a SQL server.
// The SQL server manages nearly everything related to SQL execution.
// Server 对应 Go 的 SQL server 接口，按原顺序保留三个方法。
/// SQL Server 接口：管理与 SQL 执行相关的几乎全部能力。
pub trait Server: Send + Sync {
    // GetRuntime returns the runtime for current instance.
    /// 返回当前实例的 Runtime。
    fn GetRuntime(&self) -> Arc<dyn Runtime>;

    // AcquireKSRuntime acquires a runtime handle for the target keyspace.
    // The acquired handle should be released after use.
    // this is only used in next-gen to access keyspace other than current.
    //
    /// 获取目标 keyspace 的 Runtime 句柄（仅 next-gen 跨 keyspace 访问；用后须释放）。
    fn AcquireKSRuntime(
        &self,
        targetKS: String,
        holderID: String,
    ) -> Result<Arc<dyn KSRuntimeHandle>, SqlSvrError>;

    /// 返回 DDL Owner 管理器（选主与 DDL job 调度）。
    fn GetDDLOwnerMgr(&self) -> Arc<dyn Manager>;
}

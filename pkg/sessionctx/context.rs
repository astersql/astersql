// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 会话上下文（Session Context）核心接口。
//
// 定义事务/语句执行环境、计划缓存、表锁、时间戳校验等能力边界；
// 对应 Go `sessionctx.Context`，具体实现由下游 crate 绑定。

#![allow(non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

pub use tokio_util::sync::CancellationToken as ExecutionContext;

/// Rust error boundary corresponding to Go's built-in `error` interface.
/// 对应 Go 内置 `error` 的错误边界别名。
pub type GoError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Owned Go `any` value used by both plan-cache interfaces.
/// 计划缓存使用的共享 `any` 值。
pub type SharedAny = Arc<dyn Any + Send + Sync>;

/// The value-store contract embedded by Go `planctx.Common`.
/// Go `planctx.Common` 嵌入的上下文值存储契约。
pub trait ValueStoreContext {
    fn SetValue(&mut self, key: &dyn fmt::Display, value: SharedAny);
    fn Value(&self, key: &dyn fmt::Display) -> Option<SharedAny>;
    fn ClearValue(&mut self, key: &dyn fmt::Display);
    fn GetDomain(&self) -> Option<SharedAny>;
}

/// SessionStatesHandler is the encoding/decoding contract for session state.
///
/// The associated types preserve the two package boundaries from Go without
/// coupling this API-only module to one concrete session implementation.
/// 会话状态编解码契约；关联类型保持 Go 包边界。
pub trait SessionStatesHandler {
    type SessionContext: ?Sized;
    type SessionStates;

    fn EncodeSessionStates(
        &self,
        ctx: &ExecutionContext,
        session_ctx: &Self::SessionContext,
        states: &mut Self::SessionStates,
    ) -> Result<(), GoError>;

    fn DecodeSessionStates(
        &self,
        ctx: &ExecutionContext,
        session_ctx: &Self::SessionContext,
        states: &mut Self::SessionStates,
    ) -> Result<(), GoError>;
}

/// SessionPlanCache is the prepare and non-prepared, session-local plan cache.
/// 会话级计划缓存（含 PREPARE 与非预处理）。
pub trait SessionPlanCache {
    fn Get(&self, key: &str, param_types: &dyn Any) -> Option<SharedAny>;
    fn Put(&mut self, key: String, value: SharedAny, param_types: SharedAny);
    fn Delete(&mut self, key: &str);
    fn DeleteAll(&mut self);
    fn Size(&self) -> usize;
    fn SetCapacity(&mut self, capacity: usize) -> Result<(), GoError>;
    fn Close(&mut self);
}

/// InstancePlanCache represents the instance/node-level plan cache.
/// 实例/节点级计划缓存。
pub trait InstancePlanCache {
    fn Get(&self, key: &str, param_types: &dyn Any) -> Option<SharedAny>;
    fn Put(&mut self, key: String, value: SharedAny, param_types: SharedAny) -> bool;

    /// Returned values are shared and read-only, matching the Go contract.
    /// 返回共享只读缓存项，与 Go 契约一致。
    fn All(&self) -> Vec<SharedAny>;
    fn Evict(&mut self, evict_all: bool) -> (String, usize);
    fn Size(&self) -> i64;
    fn MemUsage(&self) -> i64;
    fn GetLimits(&self) -> (i64, i64);
    fn SetLimits(&mut self, soft_limit: i64, hard_limit: i64);
}

/// The complete method set embedded from Go `planctx.Common`.
///
/// Cross-package values remain associated types. This is the same dependency
/// boundary used by the migrated `planctx` API while keeping this task's
/// native harness independent from incompatible historical Cargo lockfiles.
/// 嵌入自 Go `planctx.Common` 的完整方法集；跨包类型用关联类型表达。
pub trait PlanContextCommon: ValueStoreContext {
    type Storage: ?Sized;
    type SessionVars: ?Sized;
    type InfoSchema: ?Sized;
    type Client: ?Sized;
    type MppClient: ?Sized;
    type SessionManager: ?Sized;
    type SqlExecutor: ?Sized;
    type RestrictedSqlExecutor: ?Sized;
    type ExprContext: ?Sized;
    type RangerContext: ?Sized;
    type BuildPbContext: ?Sized;
    type TableItemId;
    type Transaction: ?Sized;

    fn GetStore(&self) -> &Self::Storage;
    fn GetSessionVars(&self) -> &Self::SessionVars;
    fn GetInfoSchema(&self) -> &Self::InfoSchema;
    fn GetLatestInfoSchema(&self) -> &Self::InfoSchema;
    fn GetLatestISWithoutSessExt(&self) -> &Self::InfoSchema;
    fn GetClient(&self) -> &Self::Client;
    fn GetMPPClient(&self) -> &Self::MppClient;
    fn GetSessionManager(&self) -> Option<&Self::SessionManager>;
    fn GetSQLExecutor(&mut self) -> &mut Self::SqlExecutor;
    fn GetRestrictedSQLExecutor(&mut self) -> &mut Self::RestrictedSqlExecutor;
    fn GetExprCtx(&self) -> &Self::ExprContext;
    fn GetRangerCtx(&self) -> &Self::RangerContext;
    fn GetBuildPBCtx(&self) -> &Self::BuildPbContext;
    fn IsCrossKS(&self) -> bool;
    fn UpdateColStatsUsage(
        &mut self,
        predicate_columns: &mut dyn Iterator<Item = Self::TableItemId>,
    );
    fn Txn(&mut self, active: bool) -> Result<Box<Self::Transaction>, GoError>;
    fn HasDirtyContent(&self, table_id: i64) -> bool;
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str);
}

/// The complete read/write table-lock contract embedded by Go `Context`.
/// 表锁读写契约（嵌入 Go Context）。
pub trait TableLockContext {
    type TableLockType;
    type TableLockInfo;

    fn CheckTableLocked(&self, table_id: i64) -> (bool, Self::TableLockType);
    fn GetAllTableLocks(&self) -> Vec<Self::TableLockInfo>;
    fn HasLockedTables(&self) -> bool;
    fn AddTableLock(&mut self, locks: &[Self::TableLockInfo]);
    fn ReleaseTableLocks(&mut self, locks: &[Self::TableLockInfo]);
    fn ReleaseTableLockByTableIDs(&mut self, table_ids: &[i64]);
    fn ReleaseAllTableLocks(&mut self);
}

/// Oracle timestamp future passed to `PrepareTSFuture`.
/// 交给 `PrepareTSFuture` 的 Oracle 时间戳 Future。
pub trait OracleFuture: Send {
    fn Wait(self: Box<Self>) -> Result<u64, GoError>;
}

/// TxnFuture owns a pending transaction and makes it valid when `Wait` returns.
/// 挂起事务的 Future；`Wait` 成功后事务生效。
pub trait TxnFuture<C: ?Sized> {
    type Transaction: ?Sized;

    fn Wait(
        &mut self,
        ctx: &ExecutionContext,
        session_ctx: &mut C,
    ) -> Result<Box<Self::Transaction>, GoError>;
}

/// Context is the transaction and statement-execution environment.
///
/// Every method from the Go interface is retained. Associated types express
/// imported interface/pointer values and let downstream integration bind the
/// already migrated package implementations without placeholder behavior.
/// 事务与语句执行环境；保留 Go 接口全部方法。
pub trait Context: PlanContextCommon + TableLockContext {
    type SessionStates;
    type SchemaValidator: ?Sized;
    type SqlServer: ?Sized;
    type TableContext: ?Sized;
    type PlanContext: ?Sized;
    type DistSqlContext: ?Sized;
    type PreparedTxnFuture: TxnFuture<Self, Transaction = Self::Transaction> + ?Sized;
    type TxnWriteThroughputSli: ?Sized;
    type StatementStats: ?Sized;
    type ProcessInfo: ?Sized;
    type SessionExtensions: ?Sized;
    type IndexUsageCollector: ?Sized;
    type CursorTracker: ?Sized;
    type CommitWaitGroup: ?Sized;

    /// 编码会话状态。
    fn EncodeStates(
        &self,
        ctx: &ExecutionContext,
        states: &mut Self::SessionStates,
    ) -> Result<(), GoError>;
    /// 解码会话状态。
    fn DecodeStates(
        &self,
        ctx: &ExecutionContext,
        states: &mut Self::SessionStates,
    ) -> Result<(), GoError>;

    /// 回滚事务。
    fn RollbackTxn(&mut self, ctx: &ExecutionContext);
    /// 提交事务。
    fn CommitTxn(&mut self, ctx: &ExecutionContext) -> Result<(), GoError>;
    fn GetSchemaValidator(&self) -> &Self::SchemaValidator;
    fn GetSQLServer(&self) -> Option<&Self::SqlServer>;
    fn GetTableCtx(&mut self) -> &mut Self::TableContext;
    fn GetPlanCtx(&mut self) -> &mut Self::PlanContext;
    fn GetDistSQLCtx(&self) -> &Self::DistSqlContext;
    /// 刷新事务上下文。
    fn RefreshTxnCtx(&mut self, ctx: &ExecutionContext) -> Result<(), GoError>;
    /// 取得会话计划缓存。
    fn GetSessionPlanCache(&mut self) -> &mut dyn SessionPlanCache;

    /// Flushes statement changes before transaction commit.
    /// 事务提交前刷写语句级变更。
    fn StmtCommit(&mut self, ctx: &ExecutionContext);

    /// Discards statement changes. `for_pessimistic_retry` is true only for
    /// pessimistic DML auto-retry, matching the Go invariant.
    /// 丢弃语句变更；仅悲观 DML 自动重试时 `for_pessimistic_retry` 为 true。
    fn StmtRollback(&mut self, ctx: &ExecutionContext, for_pessimistic_retry: bool);

    fn IsDDLOwner(&self) -> bool;
    /// 准备 Oracle 时间戳 Future。
    fn PrepareTSFuture(
        &mut self,
        ctx: &ExecutionContext,
        future: Box<dyn OracleFuture>,
        scope: &str,
    ) -> Result<(), GoError>;
    fn GetPreparedTxnFuture(&mut self) -> Option<&mut Self::PreparedTxnFuture>;
    fn GetTxnWriteThroughputSLI(&mut self) -> &mut Self::TxnWriteThroughputSli;
    fn GetBuiltinFunctionUsage(&mut self) -> &mut HashMap<String, u32>;
    fn GetStmtStats(&self) -> &Self::StatementStats;
    fn ShowProcess(&self) -> Option<&Self::ProcessInfo>;

    /// 获取命名咨询锁（Advisory Lock）。
    fn GetAdvisoryLock(&mut self, name: &str, timeout: i64) -> Result<(), GoError>;
    fn IsUsedAdvisoryLock(&self, name: &str) -> u64;
    fn ReleaseAdvisoryLock(&mut self, name: &str) -> bool;
    fn ReleaseAllAdvisoryLocks(&mut self) -> usize;

    fn GetExtensions(&self) -> Option<&Self::SessionExtensions>;
    /// 是否处于沙箱模式。
    fn InSandBoxMode(&self) -> bool;
    fn EnableSandBoxMode(&mut self);
    fn DisableSandBoxMode(&mut self);
    fn ReportUsageStats(&self);
    fn NewStmtIndexUsageCollector(&self) -> Arc<Self::IndexUsageCollector>;
    fn GetCursorTracker(&self) -> Arc<Self::CursorTracker>;
    fn GetCommitWaitGroup(&self) -> Arc<Self::CommitWaitGroup>;
    fn GetTraceCtx(&self) -> &ExecutionContext;
}

/// Context key type used for values stored on a session context.
/// 存放在会话上下文上的键类型。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BasicCtxType(i32);

impl BasicCtxType {
    /// 由整型构造键。
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// 取出底层整型值。
    pub const fn value(self) -> i32 {
        self.0
    }

    /// 键的稳定字符串名（与 Go 一致）。
    pub const fn String(self) -> &'static str {
        match self {
            QueryString => "query_string",
            Initing => "initing",
            LastExecuteDDL => "last_execute_ddl",
            _ => "unknown",
        }
    }
}

impl fmt::Display for BasicCtxType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.String())
    }
}

/// Key for the original query string.
/// 原始查询字符串键。
pub const QueryString: BasicCtxType = BasicCtxType::new(1);
/// Key indicating that the server is running a bootstrap or upgrade job.
/// 标记正在执行 bootstrap/升级任务。
pub const Initing: BasicCtxType = BasicCtxType::new(2);
/// Key indicating whether the session last executed a DDL statement.
/// 标记会话上一条是否为 DDL。
pub const LastExecuteDDL: BasicCtxType = BasicCtxType::new(3);

/// Oracle validation option. The transaction scope is always global for the
/// snapshot-read validation performed by this package.
/// Oracle 校验选项；本包快照读校验固定使用全局事务作用域。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OracleOption<'a> {
    /// 事务作用域字符串。
    pub TxnScope: &'a str,
}

/// 全局事务作用域常量。
pub const GlobalTxnScope: &str = "global";

/// Minimal callable boundary of TiKV's Oracle used by this source file.
/// TiKV Oracle 的最小可调用边界，用于校验读时间戳。
pub trait SnapshotReadOracle {
    fn ValidateReadTS(
        &self,
        ctx: &ExecutionContext,
        read_ts: u64,
        is_stale_read: bool,
        option: &OracleOption<'_>,
    ) -> Result<(), GoError>;
}

/// Storage capability required by `ValidateSnapshotReadTS`.
/// `ValidateSnapshotReadTS` 所需的存储能力。
pub trait SnapshotReadStorage {
    type Oracle: SnapshotReadOracle + ?Sized;

    fn GetOracle(&self) -> &Self::Oracle;
}

/// Strictly validates that `read_ts` does not exceed the PD timestamp.
///
/// As in Go, the call is delegated to the store's Oracle with the caller's
/// context and stale-read flag, and always uses the global transaction scope.
/// 严格校验 `read_ts` 不超过 PD 时间戳；经 store 的 Oracle 委托，固定全局作用域。
pub fn ValidateSnapshotReadTS<S: SnapshotReadStorage + ?Sized>(
    ctx: &ExecutionContext,
    store: &S,
    read_ts: u64,
    is_stale_read: bool,
) -> Result<(), GoError> {
    store.GetOracle().ValidateReadTS(
        ctx,
        read_ts,
        is_stale_read,
        &OracleOption {
            TxnScope: GlobalTxnScope,
        },
    )
}

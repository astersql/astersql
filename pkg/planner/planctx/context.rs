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

// 规划器/会话共享上下文接口：Common、PlanContext 与 BuildPBContext。
//
// Common 聚合存储、会话变量、InfoSchema、MPP/KV 客户端与事务访问；
// PlanContext 在其上扩展空值拒绝检查、事务预热与只读用户变量；
// BuildPBContext 携带将计划翻译为 tipb 执行器所需的表达式上下文与客户端。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    contextutil, exprctx, infoschema, kv, model, rangerctx, sessmgr, sqlexec, tablelock, variable,
};

/// 对应 Go 内建 `error` 接口的 Rust 错误边界类型。
/// Rust error boundary corresponding to Go's built-in `error` interface.
pub type GoError = Box<dyn std::error::Error + Send + Sync>;

/// 共享的 Go `exprctx.BuildContext` 接口值。
/// A shared Go `exprctx.BuildContext` interface value.
pub type BuildContextRef = Arc<dyn exprctx::BuildContext>;

/// 可空、共享的 Go `kv.Client` 接口值。
/// A nullable, shared Go `kv.Client` interface value.
pub type ClientRef = Arc<dyn kv::Client + Send + Sync>;

/// 共享的 Go `contextutil.WarnAppender` 接口值。
/// A shared Go `contextutil.WarnAppender` interface value.
pub type WarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>;

/// Common：计划上下文与会话上下文共享的 API 面。
///
/// Go 接口值以 trait object 引用暴露；关联的 InfoSchema 上下文与错误类型
/// 使关联类型显式化，而不在规划层固定具体 InfoSchema 实现。
///
/// Common represents the API shared by plan and session contexts.
///
/// Go interface values are exposed as trait-object references. The associated
/// InfoSchema context and error types make its associated types explicit
/// without fixing a concrete InfoSchema implementation in the planner layer.
pub trait Common: contextutil::ValueStoreContext {
    type InfoSchemaContext: ?Sized;
    type InfoSchemaError;

    /// 底层 KV Storage。
    fn GetStore(&self) -> &dyn kv::Storage;
    /// 当前会话变量（含 StmtCtx、隔离读引擎等）。
    fn GetSessionVars(&self) -> &variable::SessionVars;
    /// 语句绑定的 InfoSchema 快照（元数据只读视图）。
    fn GetInfoSchema(
        &self,
    ) -> &dyn infoschema::MetaOnlyInfoSchema<
        Context = Self::InfoSchemaContext,
        Error = Self::InfoSchemaError,
    >;
    /// 最新 InfoSchema（可能含会话扩展）。
    fn GetLatestInfoSchema(
        &self,
    ) -> &dyn infoschema::MetaOnlyInfoSchema<
        Context = Self::InfoSchemaContext,
        Error = Self::InfoSchemaError,
    >;
    /// 不含会话扩展的最新 InfoSchema。
    fn GetLatestISWithoutSessExt(
        &self,
    ) -> &dyn infoschema::MetaOnlyInfoSchema<
        Context = Self::InfoSchemaContext,
        Error = Self::InfoSchemaError,
    >;
    /// 普通 KV 客户端。
    fn GetClient(&self) -> &dyn kv::Client;
    /// MPP（Massively Parallel Processing，大规模并行处理）客户端。
    fn GetMPPClient(&self) -> &dyn kv::MPPClient;
    /// 会话管理器，可能尚未安装。
    fn GetSessionManager(&self) -> Option<&dyn sessmgr::Manager>;
    /// 常规 SQL 执行器。
    fn GetSQLExecutor(&mut self) -> &mut dyn sqlexec::SQLExecutor;
    /// 受限 SQL 执行器（系统内部语句）。
    fn GetRestrictedSQLExecutor(&mut self) -> &mut dyn sqlexec::RestrictedSQLExecutor;
    /// 表达式上下文。
    fn GetExprCtx(&self) -> &dyn exprctx::ExprContext;
    /// Ranger（索引/表扫描范围推导）上下文。
    fn GetRangerCtx(&self) -> &rangerctx::RangerContext<'_>;
    /// 构建 tipb 时使用的 PB 上下文。
    fn GetBuildPBCtx(&self) -> &BuildPBContext;
    /// 是否跨 keyspace（多租户隔离命名空间）。
    fn IsCrossKS(&self) -> bool;

    /// 更新列统计使用情况；迭代器保留 Go `iter.Seq` 的惰性逐项语义。
    /// The iterator retains Go `iter.Seq`'s lazy, one-item-at-a-time behavior.
    fn UpdateColStatsUsage(
        &mut self,
        predicate_columns: &mut dyn Iterator<Item = model::TableItemID>,
    );

    /// `active = true` 时等待挂起事务变为有效，与 Go 契约一致。
    /// With `active = true`, implementations wait for a pending transaction
    /// to become valid, matching the Go contract.
    fn Txn(&mut self, active: bool) -> Result<Box<dyn kv::Transaction>, GoError>;
    /// 表是否仍有未刷盘的脏内容。
    fn HasDirtyContent(&self, tid: i64) -> bool;

    /// 内建函数使用计数必须线程安全。
    /// Implementations must keep the underlying usage counter thread-safe.
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str);
}

/// PlanContext：构建执行计划时使用的上下文。
/// PlanContext is the context used while building a plan.
pub trait PlanContext: Common + tablelock::TableLockReadContext {
    /// 空值拒绝检查（null-reject）专用表达式上下文。
    fn GetNullRejectCheckExprCtx(&self) -> &dyn exprctx::ExprContext;
    /// 建议事务管理器做预热（提前准备事务资源）。
    fn AdviseTxnWarmup(&self) -> Result<(), GoError>;
    /// 设置只读用户变量名集合。
    fn SetReadonlyUserVarMap(&mut self, readonly_user_vars: HashMap<String, ()>);
    /// 获取只读用户变量名集合。
    fn GetReadonlyUserVarMap(&self) -> Option<&HashMap<String, ()>>;
    /// 重置扩展状态（如只读用户变量）。
    fn Reset(&mut self);
}

/// EmptyPlanContextExtended：为不需要扩展行为的 mock 提供空操作实现。
/// EmptyPlanContextExtended supplies no-op implementations to mock contexts
/// that embed it and do not need the extended PlanContext behavior.
pub struct EmptyPlanContextExtended;

impl EmptyPlanContextExtended {
    /// 空操作事务预热，始终成功。
    pub fn AdviseTxnWarmup(&self) -> Result<(), GoError> {
        Ok(())
    }

    /// 忽略只读用户变量设置。
    pub fn SetReadonlyUserVarMap(&mut self, _: HashMap<String, ()>) {}

    /// 始终返回 None。
    pub fn GetReadonlyUserVarMap(&self) -> Option<&HashMap<String, ()>> {
        None
    }

    /// 空重置。
    pub fn Reset(&mut self) {}
}

/// BuildPBContext：将计划翻译为 tipb 执行器时携带的表达式上下文与客户端等。
/// BuildPBContext carries the expression context and client used to translate
/// a plan into a tipb executor.
#[derive(Clone)]
pub struct BuildPBContext {
    /// 表达式构建上下文。
    pub ExprCtx: BuildContextRef,
    /// 可选 KV 客户端。
    pub Client: Option<ClientRef>,

    /// TiFlash 快速扫描开关。
    pub TiFlashFastScan: bool,
    /// TiFlash 细粒度 Shuffle 批大小。
    pub TiFlashFineGrainedShuffleBatchSize: u64,

    /// GROUP_CONCAT 最大长度。
    pub GroupConcatMaxLen: u64,
    /// 当前是否在 EXPLAIN 语句中。
    pub InExplainStmt: bool,
    /// 主警告处理器。
    pub WarnHandler: Option<WarnAppenderRef>,
    /// 额外警告处理器（Go 字段名拼写保留）。
    pub ExtraWarnghandler: Option<WarnAppenderRef>,
}

impl BuildPBContext {
    /// 克隆表达式上下文引用。
    pub fn GetExprCtx(&self) -> BuildContextRef {
        Arc::clone(&self.ExprCtx)
    }

    /// 克隆可选客户端引用。
    pub fn GetClient(&self) -> Option<ClientRef> {
        self.Client.clone()
    }

    /// Detach：浅拷贝结构体并仅替换 ExprCtx；StatementContext 仍不可并发复用。
    /// Detach shallow-copies the Go struct and replaces only ExprCtx.
    /// StatementContext remains unsafe to reuse concurrently, as in Go.
    pub fn Detach(&self, static_expr_ctx: BuildContextRef) -> Box<Self> {
        let mut new_ctx = self.clone();
        new_ctx.ExprCtx = static_expr_ctx;
        Box::new(new_ctx)
    }
}

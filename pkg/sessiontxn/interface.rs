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

#![allow(non_snake_case, non_upper_case_globals)]

// 会话事务管理接口：TxnManager / TxnContextProvider 及进入新事务的请求类型。
//
// 对应 Go `sessiontxn` 包的核心抽象。事务生命周期（开启、语句起止、悲观锁、
// 错误重试、提交前选项）通过 provider 回调驱动；读时间戳（ReadTS）与
// ForUpdateTS 决定 MVCC 可见性与加锁语义。
// `TxnManager` 统一持有当前 provider，对上层屏蔽不同隔离级别的实现差异。

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use astersql_infoschema as infoschema;
use astersql_kv as kv;
use astersql_parser_ast as ast;
use astersql_sessionctx as sessionctx;

/// 统一错误类型别名（映射 Go error）。
pub type Error = sessionctx::GoError;
/// 请求执行上下文。
pub type RequestContext = sessionctx::ExecutionContext;
/// InfoSchema 引用：描述当前可见的库表元数据快照。
pub type InfoSchemaRef = Arc<dyn infoschema::InfoSchema>;
/// KV 只读快照。
pub type Snapshot = Box<dyn kv::Snapshot>;
/// KV 事务句柄。
pub type Transaction = Box<dyn kv::Transaction>;
/// SQL 语句 AST 节点。
pub type Statement = Rc<dyn ast::ast::StmtNode>;

/// 进入新事务的入口类型，决定初始化路径与 provider 行为。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EnterNewTxnType {
    #[default]
    /// 默认进入新事务。
    EnterNewTxnDefault,
    /// 由显式 BEGIN 语句触发。
    EnterNewTxnWithBeginStmt,
    /// 在语句执行前隐式开启事务。
    EnterNewTxnBeforeStmt,
    /// 用新的 TxnContextProvider 替换当前 provider。
    EnterNewTxnWithReplaceProvider,
}

pub use EnterNewTxnType::{
    EnterNewTxnBeforeStmt, EnterNewTxnDefault, EnterNewTxnWithBeginStmt,
    EnterNewTxnWithReplaceProvider,
};

/// 进入新事务的请求参数：类型、可选 provider、事务模式与一致性/陈旧读配置。
pub struct EnterNewTxnRequest {
    pub Type: EnterNewTxnType,
    pub Provider: Option<Box<dyn TxnContextProvider>>,
    /// 事务模式字符串，如乐观（Optimistic）/悲观（Pessimistic）。
    pub TxnMode: String,
    /// 仅要求因果一致性（Causal Consistency），可降低跨 Region 同步成本。
    pub CausalConsistencyOnly: bool,
    /// 陈旧读时间戳（Stale Read TS）；非 0 时按历史版本读取。
    pub StaleReadTS: u64,
}

impl Default for EnterNewTxnRequest {
    fn default() -> Self {
        Self {
            Type: EnterNewTxnDefault,
            Provider: None,
            TxnMode: String::new(),
            CausalConsistencyOnly: false,
            StaleReadTS: 0,
        }
    }
}

/// 语句错误处理阶段：查询完成后，或悲观锁加锁失败后。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StmtErrorHandlePoint {
    StmtErrAfterQuery,
    StmtErrAfterPessimisticLock,
}

pub use StmtErrorHandlePoint::{StmtErrAfterPessimisticLock, StmtErrAfterQuery};

/// 对语句错误的处置建议：直接报错、已准备好重试、或暂无明确意见。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StmtErrorAction {
    StmtActionError,
    StmtActionRetryReady,
    StmtActionNoIdea,
}

pub use StmtErrorAction::{StmtActionError, StmtActionNoIdea, StmtActionRetryReady};

/// 错误处置建议：(动作, 可选包装错误)。
pub type StmtErrorAdvice = (StmtErrorAction, Option<Error>);

/// 构造“直接以该错误失败”的建议。
pub fn ErrorAction(error: Error) -> StmtErrorAdvice {
    (StmtActionError, Some(error))
}

/// 构造“可立即重试”的建议。
pub fn RetryReady() -> StmtErrorAdvice {
    (StmtActionRetryReady, None)
}

/// 构造“暂无明确处置意见”的建议。
pub fn NoIdea() -> StmtErrorAdvice {
    (StmtActionNoIdea, None)
}

/// 事务侧可接受的优化建议：预热与基于执行计划的优化。
pub trait TxnAdvisable {
    /// 预热：提前准备快照/时间戳等，降低首条语句延迟。
    fn AdviseWarmup(&mut self) -> Result<(), Error>;
    /// 结合执行计划（Plan）做事务路径优化。
    fn AdviseOptimizeWithPlan(&mut self, plan: &dyn Any) -> Result<(), Error>;
}

/// 事务上下文提供者：隔离级别相关的 TS/快照获取与语句生命周期回调。
pub trait TxnContextProvider: TxnAdvisable {
    fn GetTxnInfoSchema(&self) -> InfoSchemaRef;
    fn GetTxnScope(&self) -> String;
    fn GetReadReplicaScope(&self) -> String;
    /// 获取当前语句读时间戳（决定 MVCC 可见版本）。
    fn GetStmtReadTS(&mut self) -> Result<u64, Error>;
    /// 获取 ForUpdate 时间戳（悲观锁/SELECT FOR UPDATE 使用）。
    fn GetStmtForUpdateTS(&mut self) -> Result<u64, Error>;
    fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, Error>;
    fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, Error>;
    fn OnInitialize(&mut self, ctx: &RequestContext, kind: EnterNewTxnType) -> Result<(), Error>;
    fn OnStmtStart(&mut self, ctx: &RequestContext, node: Option<Statement>) -> Result<(), Error>;
    fn OnPessimisticStmtStart(&mut self, ctx: &RequestContext) -> Result<(), Error>;
    fn OnPessimisticStmtEnd(&mut self, ctx: &RequestContext, successful: bool)
    -> Result<(), Error>;
    fn OnStmtErrorForNextAction(
        &mut self,
        ctx: &RequestContext,
        point: StmtErrorHandlePoint,
        error: Error,
    ) -> StmtErrorAdvice;
    fn OnStmtRetry(&mut self, ctx: &RequestContext) -> Result<(), Error>;
    fn OnStmtCommit(&mut self, ctx: &RequestContext) -> Result<(), Error>;
    fn OnStmtRollback(
        &mut self,
        ctx: &RequestContext,
        pessimistic_retry: bool,
    ) -> Result<(), Error>;
    fn OnLocalTemporaryTableCreated(&mut self);
    fn ActivateTxn(&mut self) -> Result<Transaction, Error>;
    /// 提交前设置事务选项，并可选校验 CommitTS。
    fn SetOptionsBeforeCommit(
        &mut self,
        txn: &mut dyn kv::Transaction,
        commit_ts_checker: &dyn Fn(u64) -> bool,
    ) -> Result<(), Error>;
}

/// 会话级事务管理器：对外统一转发到当前 TxnContextProvider，并管理进入/结束事务。
pub trait TxnManager: TxnAdvisable {
    fn GetTxnInfoSchema(&self) -> InfoSchemaRef;
    fn GetTxnScope(&self) -> String;
    fn GetReadReplicaScope(&self) -> String;
    fn GetStmtReadTS(&mut self) -> Result<u64, Error>;
    fn GetStmtForUpdateTS(&mut self) -> Result<u64, Error>;
    fn GetContextProvider(&mut self) -> &mut dyn TxnContextProvider;
    fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, Error>;
    fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, Error>;
    fn EnterNewTxn(
        &mut self,
        ctx: &RequestContext,
        request: &mut EnterNewTxnRequest,
    ) -> Result<(), Error>;
    fn OnTxnEnd(&mut self);
    fn OnStmtStart(&mut self, ctx: &RequestContext, node: Option<Statement>) -> Result<(), Error>;
    fn OnPessimisticStmtStart(&mut self, ctx: &RequestContext) -> Result<(), Error>;
    fn OnPessimisticStmtEnd(&mut self, ctx: &RequestContext, successful: bool)
    -> Result<(), Error>;
    fn OnStmtErrorForNextAction(
        &mut self,
        ctx: &RequestContext,
        point: StmtErrorHandlePoint,
        error: Error,
    ) -> StmtErrorAdvice;
    fn OnStmtRetry(&mut self, ctx: &RequestContext) -> Result<(), Error>;
    fn OnStmtCommit(&mut self, ctx: &RequestContext) -> Result<(), Error>;
    fn OnStmtRollback(
        &mut self,
        ctx: &RequestContext,
        pessimistic_retry: bool,
    ) -> Result<(), Error>;
    fn OnStmtEnd(&mut self);
    fn OnLocalTemporaryTableCreated(&mut self);
    fn ActivateTxn(&mut self) -> Result<Transaction, Error>;
    fn GetCurrentStmt(&self) -> Option<Statement>;
    fn SetOptionsBeforeCommit(
        &mut self,
        txn: &mut dyn kv::Transaction,
        commit_ts_checker: &dyn Fn(u64) -> bool,
    ) -> Result<(), Error>;
}

/// Adapter boundary replacing Go's package-level injected function. A real
/// session owns its manager and exposes the same one for the session lifetime.
///
/// 替代 Go 包级注入函数：真实会话在整个生命周期持有并暴露同一 TxnManager。
pub trait TxnManagerContext {
    fn txn_manager(&mut self) -> &mut dyn TxnManager;
}

/// 从会话上下文取出事务管理器。
pub fn GetTxnManager<C: TxnManagerContext + ?Sized>(sctx: &mut C) -> &mut dyn TxnManager {
    sctx.txn_manager()
}

/// 先按执行计划优化事务路径，再执行预热。
pub fn AdviseOptimizeWithPlanAndThenWarmUp<C: TxnManagerContext + ?Sized>(
    sctx: &mut C,
    plan: &dyn Any,
) -> Result<(), Error> {
    let manager = GetTxnManager(sctx);
    manager.AdviseOptimizeWithPlan(plan)?;
    manager.AdviseWarmup()
}

/// 以默认类型、乐观事务模式进入新事务。
pub fn NewTxn<C: TxnManagerContext + ?Sized>(
    ctx: &RequestContext,
    sctx: &mut C,
) -> Result<(), Error> {
    let mut request = EnterNewTxnRequest {
        Type: EnterNewTxnDefault,
        TxnMode: ast::Optimistic.to_owned(),
        ..EnterNewTxnRequest::default()
    };
    GetTxnManager(sctx).EnterNewTxn(ctx, &mut request)
}

/// 在语句执行路径中开启新事务；若已有当前语句则立即触发 `OnStmtStart`。
pub fn NewTxnInStmt<C: TxnManagerContext + ?Sized>(
    ctx: &RequestContext,
    sctx: &mut C,
) -> Result<(), Error> {
    NewTxn(ctx, sctx)?;
    let manager = GetTxnManager(sctx);
    let statement = manager.GetCurrentStmt();
    manager.OnStmtStart(ctx, statement)
}

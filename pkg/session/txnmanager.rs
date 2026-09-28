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

// 会话事务管理器（TxnManager）。
//
// 负责进入新事务、选择乐观/悲观 Provider、转发语句生命周期钩子，
// 并记录慢事务事件时间线。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{SessionError, SessionResult};

/// 全局事务作用域（跨副本一致性读默认范围）。
pub const GlobalTxnScope: &str = "global";
/// 全局读副本作用域。
pub const GlobalReplicaScope: &str = "global";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 进入新事务的方式。
pub enum EnterNewTxnType {
    #[default]
    /// 隐式进入（自动开启）。
    Default,
    /// 显式 BEGIN 语句进入。
    WithBeginStmt,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 事务模式：乐观或悲观。
pub enum TxnMode {
    #[default]
    /// 乐观事务：冲突在提交阶段检测。
    Optimistic,
    /// 悲观事务：执行阶段加锁。
    Pessimistic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// SQL 隔离级别。
pub enum IsolationLevel {
    /// 读已提交。
    ReadCommitted,
    /// 可串行化。
    Serializable,
    #[default]
    /// 可重复读（默认）。
    RepeatableRead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 语句错误处理切入点。
pub enum StmtErrorHandlePoint {
    /// 悲观加锁之后。
    AfterPessimisticLock,
    /// 查询执行之后。
    AfterQuery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 对语句错误的下一步动作建议。
pub enum StmtErrorAction {
    /// 无明确建议。
    NoIdea,
    /// 可重试。
    RetryReady,
    /// 应向上返回错误。
    Error,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 当前语句节点（保留原始 SQL 文本）。
pub struct StatementNode {
    /// 原始 SQL 文本。
    pub original_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 事务时间线中的一个事件及其距上一事件的时长。
pub struct Event {
    /// 事件名。
    pub event: String,
    /// 距上一事件的时长。
    pub duration: Duration,
}

/// 具体事务上下文提供者：实现隔离级别/模式相关的 TS 与钩子。
pub trait TxnContextProvider: Send {
    fn OnInitialize(&mut self, enter_type: EnterNewTxnType) -> SessionResult;
    fn GetTxnInfoSchema(&self) -> Option<String>;
    fn GetTxnScope(&self) -> String;
    fn GetReadReplicaScope(&self) -> String;
    fn GetStmtReadTS(&self) -> SessionResult<u64>;
    fn GetStmtForUpdateTS(&self) -> SessionResult<u64>;
    fn GetSnapshotWithStmtReadTS(&self) -> SessionResult<String>;
    fn GetSnapshotWithStmtForUpdateTS(&self) -> SessionResult<String>;
    fn OnStmtStart(&mut self, statement: Option<&StatementNode>) -> SessionResult;
    fn OnStmtCommit(&mut self) -> SessionResult;
    fn OnStmtRollback(&mut self, pessimistic_retry: bool) -> SessionResult;
    fn OnPessimisticStmtStart(&mut self) -> SessionResult;
    fn OnPessimisticStmtEnd(&mut self, successful: bool) -> SessionResult;
    fn OnStmtErrorForNextAction(
        &mut self,
        point: StmtErrorHandlePoint,
        error: &SessionError,
    ) -> SessionResult<StmtErrorAction>;
    fn ActivateTxn(&mut self) -> SessionResult<String>;
    fn OnStmtRetry(&mut self) -> SessionResult;
    fn OnLocalTemporaryTableCreated(&mut self);
    fn AdviseWarmup(&mut self) -> SessionResult;
    fn AdviseOptimizeWithPlan(&mut self, plan: &dyn std::any::Any) -> SessionResult;
    fn SetOptionsBeforeCommit(&mut self, commit_ts_checker: &dyn Fn(u64) -> bool) -> SessionResult;
}

/// 进入新事务的请求参数。
pub struct EnterNewTxnRequest {
    /// 进入方式。
    pub Type: EnterNewTxnType,
    /// 可选预构造 Provider；有则直接使用。
    pub Provider: Option<Box<dyn TxnContextProvider>>,
    /// 陈旧读时间戳；>0 时走 stale-read Provider。
    pub StaleReadTS: u64,
    /// 指定事务模式；None 则用会话默认。
    pub TxnMode: Option<TxnMode>,
    /// 是否仅要求因果一致性。
    pub CausalConsistencyOnly: bool,
}

/// TxnManager 对会话能力的依赖接口。
pub trait TxnManagerSession: Send + Sync {
    fn LatestInfoSchema(&self) -> Option<String>;
    fn DefaultTxnMode(&self) -> TxnMode;
    fn IsolationLevelForNewTxn(&self) -> IsolationLevel;
    fn SetStaleReadTS(&self, timestamp: u64);
    fn SetInTxn(&self, in_transaction: bool);
    fn RollbackTxn(&self);
    fn BulkDMLEnabled(&self) -> bool;
    /// Go `SessionVars.EnableRedactLog`；默认关闭以保持现有实现兼容。
    fn EnableRedactLog(&self) -> &str {
        "OFF"
    }
    fn SlowTxnThresholdMs(&self) -> u64;
    fn ConnectionID(&self) -> u64;
    fn TxnStartTS(&self) -> u64;
    fn TxnStatementCount(&self) -> u64;
    fn TraceTxnEnter(&self, enter_type: EnterNewTxnType, explicit: bool);
    fn TraceTxnEnd(&self, duration: Duration, slow: bool);
    fn LogSlowTxn(&self, duration: Duration, events: &[Event]);
}

/// 按模式/隔离级别创建 TxnContextProvider 的工厂。
pub trait TxnProviderFactory: Send + Sync {
    fn NewStaleReadProvider(&self, timestamp: u64) -> SessionResult<Box<dyn TxnContextProvider>>;
    fn NewOptimisticProvider(
        &self,
        slot: usize,
        causal_consistency_only: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>>;
    fn NewPessimisticRCProvider(
        &self,
        causal_consistency_only: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>>;
    fn NewPessimisticSerializableProvider(
        &self,
        causal_consistency_only: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>>;
    fn NewPessimisticRRProvider(
        &self,
        causal_consistency_only: bool,
    ) -> SessionResult<Box<dyn TxnContextProvider>>;
}

/// 会话级事务管理器：持有当前 Provider 与慢事务事件缓冲。
pub struct TxnManager {
    session: Arc<dyn TxnManagerSession>,
    factory: Arc<dyn TxnProviderFactory>,
    provider: Option<Box<dyn TxnContextProvider>>,
    stmtNode: Option<StatementNode>,
    events: Vec<Event>,
    lastInstant: Instant,
    enterTxnInstant: Instant,
    optimistic_slot: usize,
}

/// 构造 TxnManager（Go 风格命名入口）。
pub fn getTxnManager(
    session: Arc<dyn TxnManagerSession>,
    factory: Arc<dyn TxnProviderFactory>,
) -> TxnManager {
    TxnManager::new(session, factory)
}

impl TxnManager {
    /// 创建管理器，初始无活动 Provider。
    pub fn new(session: Arc<dyn TxnManagerSession>, factory: Arc<dyn TxnProviderFactory>) -> Self {
        Self {
            session,
            factory,
            provider: None,
            stmtNode: None,
            events: Vec::with_capacity(10),
            lastInstant: Instant::now(),
            enterTxnInstant: Instant::now(),
            optimistic_slot: 0,
        }
    }

    /// 取事务绑定的信息模式；无则回落到会话最新。
    pub fn GetTxnInfoSchema(&self) -> Option<String> {
        match self.provider.as_ref() {
            Some(provider) => provider.GetTxnInfoSchema(),
            None => self.session.LatestInfoSchema(),
        }
    }

    /// 返回当前上下文 Provider。
    pub fn GetContextProvider(&self) -> Option<&dyn TxnContextProvider> {
        self.provider.as_deref()
    }

    /// 事务作用域；无 Provider 时为全局。
    pub fn GetTxnScope(&self) -> String {
        self.provider
            .as_ref()
            .map(|provider| provider.GetTxnScope())
            .unwrap_or_else(|| GlobalTxnScope.to_owned())
    }

    /// 读副本作用域；无 Provider 时为全局。
    pub fn GetReadReplicaScope(&self) -> String {
        self.provider
            .as_ref()
            .map(|provider| provider.GetReadReplicaScope())
            .unwrap_or_else(|| GlobalReplicaScope.to_owned())
    }

    /// 当前语句读时间戳。
    pub fn GetStmtReadTS(&self) -> SessionResult<u64> {
        self.provider_ref()?.GetStmtReadTS()
    }

    /// 当前语句 FOR UPDATE 时间戳。
    pub fn GetStmtForUpdateTS(&self) -> SessionResult<u64> {
        self.provider_ref()?.GetStmtForUpdateTS()
    }

    /// 基于语句读 TS 的快照标识。
    pub fn GetSnapshotWithStmtReadTS(&self) -> SessionResult<String> {
        self.provider_ref()?.GetSnapshotWithStmtReadTS()
    }

    /// 基于 FOR UPDATE TS 的快照标识。
    pub fn GetSnapshotWithStmtForUpdateTS(&self) -> SessionResult<String> {
        self.provider_ref()?.GetSnapshotWithStmtForUpdateTS()
    }

    /// 进入新事务：创建并初始化 Provider，记录进入事件。
    pub fn EnterNewTxn(&mut self, request: &mut EnterNewTxnRequest) -> SessionResult {
        let mut provider = self.newProviderWithRequest(request)?;
        // 初始化失败则回滚会话事务并返回错误。
        if let Err(error) = provider.OnInitialize(request.Type) {
            self.session.RollbackTxn();
            return Err(error);
        }
        if request.Type == EnterNewTxnType::WithBeginStmt {
            self.session.SetInTxn(true);
        }
        self.provider = Some(provider);
        self.session
            .TraceTxnEnter(request.Type, request.Type == EnterNewTxnType::WithBeginStmt);
        self.resetEvents();
        self.recordEvent("enter txn");
        Ok(())
    }

    /// 事务结束：清理 Provider，统计时长并可能记录慢事务。
    pub fn OnTxnEnd(&mut self) {
        self.provider = None;
        self.stmtNode = None;
        self.recordEvent("txn end");
        let duration = self.enterTxnInstant.elapsed();
        let threshold = self.session.SlowTxnThresholdMs();
        let slow = threshold > 0 && duration.as_millis() as u64 >= threshold;
        self.session.TraceTxnEnd(duration, slow);
        // 超过阈值则输出慢事务日志（含事件时间线）。
        if slow {
            self.session.LogSlowTxn(duration, &self.events);
        }
        self.lastInstant = Instant::now();
    }

    /// 当前语句节点。
    pub fn GetCurrentStmt(&self) -> Option<&StatementNode> {
        self.stmtNode.as_ref()
    }

    /// 语句开始：规范化 SQL 记入事件并转发给 Provider。
    pub fn OnStmtStart(&mut self, statement: Option<StatementNode>) -> SessionResult {
        self.stmtNode = statement;
        if self.provider.is_none() {
            return Err(SessionError::new("context provider not set"));
        }
        let event = self
            .stmtNode
            .as_ref()
            .map(|node| {
                astersql_parser::Normalize(&node.original_text, self.session.EnableRedactLog())
            })
            .unwrap_or_default();
        self.recordEvent(&event);
        let statement = self.stmtNode.clone();
        self.provider_mut()?.OnStmtStart(statement.as_ref())
    }

    /// 语句结束事件打点。
    pub fn OnStmtEnd(&mut self) {
        self.recordEvent("stmt end");
    }

    /// 悲观语句开始。
    pub fn OnPessimisticStmtStart(&mut self) -> SessionResult {
        self.provider_mut()?.OnPessimisticStmtStart()
    }

    /// 悲观语句结束。
    pub fn OnPessimisticStmtEnd(&mut self, successful: bool) -> SessionResult {
        self.provider_mut()?.OnPessimisticStmtEnd(successful)
    }

    /// 根据错误决定下一步动作；无 Provider 时返回 NoIdea。
    pub fn OnStmtErrorForNextAction(
        &mut self,
        point: StmtErrorHandlePoint,
        error: &SessionError,
    ) -> SessionResult<StmtErrorAction> {
        let Some(provider) = self.provider.as_mut() else {
            return Ok(StmtErrorAction::NoIdea);
        };
        provider.OnStmtErrorForNextAction(point, error)
    }

    /// 激活惰性事务，返回激活后的标识。
    pub fn ActivateTxn(&mut self) -> SessionResult<String> {
        self.provider_mut()?.ActivateTxn()
    }

    /// 语句重试钩子。
    pub fn OnStmtRetry(&mut self) -> SessionResult {
        self.provider_mut()?.OnStmtRetry()
    }

    /// 语句提交钩子。
    pub fn OnStmtCommit(&mut self) -> SessionResult {
        if self.provider.is_none() {
            return Err(SessionError::new("context provider not set"));
        }
        self.recordEvent("stmt commit");
        self.provider_mut()?.OnStmtCommit()
    }

    /// 语句回滚钩子；`pessimistic_retry` 表示悲观自动重试路径。
    pub fn OnStmtRollback(&mut self, pessimistic_retry: bool) -> SessionResult {
        if self.provider.is_none() {
            return Err(SessionError::new("context provider not set"));
        }
        self.recordEvent("stmt rollback");
        self.provider_mut()?.OnStmtRollback(pessimistic_retry)
    }

    /// 本地临时表创建后通知 Provider。
    pub fn OnLocalTemporaryTableCreated(&mut self) {
        if let Some(provider) = self.provider.as_mut() {
            provider.OnLocalTemporaryTableCreated();
        }
    }

    /// 建议预热；Bulk DML 开启时跳过。
    pub fn AdviseWarmup(&mut self) -> SessionResult {
        if self.session.BulkDMLEnabled() {
            return Ok(());
        }
        match self.provider.as_mut() {
            Some(provider) => provider.AdviseWarmup(),
            None => Ok(()),
        }
    }

    /// 基于执行计划给出优化建议。
    pub fn AdviseOptimizeWithPlan(&mut self, plan: &dyn std::any::Any) -> SessionResult {
        match self.provider.as_mut() {
            Some(provider) => provider.AdviseOptimizeWithPlan(plan),
            None => Ok(()),
        }
    }

    /// 提交前设置选项（含 commit TS 检查回调）。
    pub fn SetOptionsBeforeCommit(
        &mut self,
        commit_ts_checker: &dyn Fn(u64) -> bool,
    ) -> SessionResult {
        self.provider_mut()?
            .SetOptionsBeforeCommit(commit_ts_checker)
    }

    /// 追加事件并刷新计时起点。
    fn recordEvent(&mut self, event: &str) {
        self.events.push(Event {
            event: event.to_owned(),
            duration: self.lastInstant.elapsed(),
        });
        self.lastInstant = Instant::now();
    }

    /// 清空事件并重置进入事务时刻。
    fn resetEvents(&mut self) {
        self.events.clear();
        self.enterTxnInstant = Instant::now();
        self.lastInstant = self.enterTxnInstant;
    }

    /// 按请求选择/创建 Provider（预置、陈旧读、乐观或悲观各隔离级别）。
    fn newProviderWithRequest(
        &mut self,
        request: &mut EnterNewTxnRequest,
    ) -> SessionResult<Box<dyn TxnContextProvider>> {
        // 调用方已提供 Provider 则直接使用。
        if let Some(provider) = request.Provider.take() {
            return Ok(provider);
        }
        // 陈旧读：按历史时间戳读快照。
        if request.StaleReadTS > 0 {
            self.session.SetStaleReadTS(request.StaleReadTS);
            return self.factory.NewStaleReadProvider(request.StaleReadTS);
        }
        let mode = request
            .TxnMode
            .unwrap_or_else(|| self.session.DefaultTxnMode());
        match mode {
            // 乐观 Provider 在两个 slot 间翻转，便于复用内部缓冲。
            TxnMode::Optimistic => {
                let slot = self.optimistic_slot;
                self.optimistic_slot ^= 1;
                self.factory
                    .NewOptimisticProvider(slot, request.CausalConsistencyOnly)
            }
            TxnMode::Pessimistic => match self.session.IsolationLevelForNewTxn() {
                IsolationLevel::ReadCommitted => self
                    .factory
                    .NewPessimisticRCProvider(request.CausalConsistencyOnly),
                IsolationLevel::Serializable => self
                    .factory
                    .NewPessimisticSerializableProvider(request.CausalConsistencyOnly),
                IsolationLevel::RepeatableRead => self
                    .factory
                    .NewPessimisticRRProvider(request.CausalConsistencyOnly),
            },
        }
    }

    /// 取 Provider 不可变引用；未设置则报错。
    fn provider_ref(&self) -> SessionResult<&dyn TxnContextProvider> {
        self.provider
            .as_deref()
            .ok_or_else(|| SessionError::new("context provider not set"))
    }

    /// 取 Provider 可变引用；未设置则报错。
    fn provider_mut(&mut self) -> SessionResult<&mut (dyn TxnContextProvider + 'static)> {
        self.provider
            .as_deref_mut()
            .ok_or_else(|| SessionError::new("context provider not set"))
    }
}

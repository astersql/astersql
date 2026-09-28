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

// 过期读事务上下文 Provider：在固定读 ts 上提供只读快照事务。
//
// 对应 Go `StalenessTxnContextProvider`：激活只读事务、提供语句读 ts /
// 快照，并拒绝 ForUpdateTS。读副本作用域固定为 global，保证跨副本
// 一致的历史读视图。

use std::sync::Arc;

use crate::{
    Context, Error, ErrorKind, InfoSchema, SessionRef, Snapshot, Transaction, TransactionContext,
    get_session_snapshot_info_schema,
};

/// 全局事务作用域：过期读默认跨副本一致读范围。
pub const GLOBAL_TXN_SCOPE: &str = "global";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 进入新事务的方式（对应 TxnManager 的 enter 类型）。
pub enum EnterNewTxnType {
    /// 默认隐式进入。
    Default,
    /// 显式 BEGIN 进入。
    WithBeginStatement,
    /// 仅替换 Provider，不重新激活底层事务。
    WithReplaceProvider,
    /// 不支持的进入类型。
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 语句错误后的建议动作（过期读 Provider 恒为 NoIdea）。
pub enum StatementErrorAction {
    NoIdea,
}

/// 过期读专用事务上下文 Provider：固定 ts 的只读快照事务。
pub struct StalenessTxnContextProvider {
    context: Context,
    session: SessionRef,
    info_schema: Option<InfoSchema>,
    ts: u64,
    transaction: Option<Transaction>,
}

impl StalenessTxnContextProvider {
    /// 构造尚未安装到会话事务管理器的 Provider。
    pub fn new(session: SessionRef, ts: u64, info_schema: Option<InfoSchema>) -> Self {
        Self {
            context: Context,
            session,
            info_schema,
            ts,
            transaction: None,
        }
    }

    /// 当前事务 InfoSchema。
    pub fn txn_info_schema(&self) -> Option<&InfoSchema> {
        self.info_schema.as_ref()
    }

    /// 事务作用域（来自会话 txn_context）。
    pub fn txn_scope(&self) -> String {
        self.session
            .lock()
            .ok()
            .and_then(|session| {
                session
                    .txn_context
                    .as_ref()
                    .map(|context| context.txn_scope.clone())
            })
            .unwrap_or_default()
    }

    /// 读副本作用域：读取配置，默认是 global。
    pub fn read_replica_scope(&self) -> String {
        self.session
            .lock()
            .map(|session| session.txn_scope_config.clone())
            .unwrap_or_else(|_| GLOBAL_TXN_SCOPE.to_owned())
    }

    /// 返回语句读 ts；autocommit=0 且未进事务时自动激活过期读事务。
    pub fn stmt_read_ts(&mut self) -> Result<u64, Error> {
        let should_activate = {
            let session = self
                .session
                .lock()
                .map_err(|_| Error::backend("session lock poisoned"))?;
            !session.autocommit && !session.in_txn
        };
        // 非 autocommit 且尚未 in_txn：先激活只读过期读事务。
        if should_activate {
            self.activate_txn()?;
            self.session
                .lock()
                .map_err(|_| Error::backend("session lock poisoned"))?
                .in_txn = true;
        }
        Ok(self.ts)
    }

    /// 过期读不支持 ForUpdateTS。
    pub fn stmt_for_update_ts(&self) -> Result<u64, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "GetForUpdateTS not supported for stalenessTxnProvider",
        ))
    }

    /// 按进入类型初始化：激活事务或仅替换 Provider 上下文。
    pub fn on_initialize(
        &mut self,
        context: Context,
        enter_type: EnterNewTxnType,
    ) -> Result<(), Error> {
        self.context = context;
        match enter_type {
            EnterNewTxnType::Default | EnterNewTxnType::WithBeginStatement => {
                self.activate_stale_txn()
            }
            EnterNewTxnType::WithReplaceProvider => {
                self.enter_new_stale_txn_with_replace_provider()
            }
            EnterNewTxnType::Unsupported => Err(Error::new(
                ErrorKind::Unsupported,
                "unsupported enter-new-txn type",
            )),
        }
    }

    /// 提交旧事务（如有）并以固定 ts 创建只读过期读事务与上下文。
    fn activate_stale_txn(&mut self) -> Result<(), Error> {
        let backend = Arc::clone(
            &self
                .session
                .lock()
                .map_err(|_| Error::backend("session lock poisoned"))?
                .backend,
        );
        // 进入新事务前先落盘/清理当前事务（与 Go ActivateStaleTxn 一致）。
        backend.commit_before_enter_new_txn()?;
        let mut transaction = backend.create_transaction(self.ts)?;
        transaction.staleness_read_only = true;
        transaction.txn_scope = GLOBAL_TXN_SCOPE.into();
        let info_schema = get_session_snapshot_info_schema(&self.session, self.ts)?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?;
        transaction.assertion_level = session.assertion_level;
        transaction.shard_allocate_step = session.shard_allocate_step;
        transaction.temporary_table_interceptor = info_schema.local_temporary_tables_attached;
        transaction.snapshot.staleness_read_only = true;
        transaction.snapshot.temporary_table_interceptor =
            info_schema.local_temporary_tables_attached;
        session.txn_context = Some(TransactionContext {
            info_schema: info_schema.clone(),
            start_ts: transaction.start_ts,
            is_staleness: true,
            txn_scope: GLOBAL_TXN_SCOPE.into(),
        });
        session.active_transaction = Some(transaction.clone());
        session.provider_is_staleness = true;
        session.snapshot_system_variable.clear();
        self.info_schema = Some(info_schema);
        Ok(())
    }

    /// 替换 Provider：只更新 txn_context，不 create_transaction。
    fn enter_new_stale_txn_with_replace_provider(&mut self) -> Result<(), Error> {
        if self.info_schema.is_none() {
            self.info_schema = Some(get_session_snapshot_info_schema(&self.session, self.ts)?);
        }
        let mut session = self
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?;
        let context = session
            .txn_context
            .get_or_insert_with(TransactionContext::default);
        context.txn_scope = GLOBAL_TXN_SCOPE.into();
        context.is_staleness = true;
        context.info_schema = self.info_schema.clone().expect("info schema initialized");
        session.provider_is_staleness = true;
        Ok(())
    }

    /// 语句开始钩子：刷新上下文。
    pub fn on_stmt_start(&mut self, context: Context) -> Result<(), Error> {
        self.context = context;
        Ok(())
    }

    /// 悲观语句开始：过期读无额外动作。
    pub fn on_pessimistic_stmt_start(&self) -> Result<(), Error> {
        Ok(())
    }

    /// 悲观语句结束：过期读无额外动作。
    pub fn on_pessimistic_stmt_end(&self, _success: bool) -> Result<(), Error> {
        Ok(())
    }

    /// 惰性激活并缓存活跃事务句柄。
    pub fn activate_txn(&mut self) -> Result<Transaction, Error> {
        // 已激活则直接复用。
        if let Some(transaction) = &self.transaction {
            return Ok(transaction.clone());
        }
        self.activate_stale_txn()?;
        let transaction = self
            .session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .active_transaction
            .clone()
            .ok_or_else(|| Error::backend("transaction activation returned no transaction"))?;
        self.transaction = Some(transaction.clone());
        Ok(transaction)
    }

    /// 语句错误后无重试建议。
    pub fn on_stmt_error_for_next_action(
        &self,
        _error: &Error,
    ) -> (StatementErrorAction, Option<Error>) {
        (StatementErrorAction::NoIdea, None)
    }

    /// 语句重试钩子。
    pub fn on_stmt_retry(&mut self, context: Context) -> Result<(), Error> {
        self.context = context;
        Ok(())
    }

    /// 语句提交钩子（过期读无额外逻辑）。
    pub fn on_stmt_commit(&self) -> Result<(), Error> {
        Ok(())
    }

    /// 语句回滚钩子（过期读无额外逻辑）。
    pub fn on_stmt_rollback(&self, _is_pessimistic: bool) -> Result<(), Error> {
        Ok(())
    }

    /// 预热建议占位。
    pub fn advise_warmup(&self) -> Result<(), Error> {
        Ok(())
    }

    /// 基于执行计划的优化建议占位。
    pub fn advise_optimize_with_plan<T>(&self, _plan: &T) -> Result<(), Error> {
        Ok(())
    }

    /// 按语句读 ts 取快照：优先复用活跃事务快照，否则向后端要快照。
    pub fn snapshot_with_stmt_read_ts(&mut self) -> Result<Snapshot, Error> {
        self.stmt_read_ts()?;
        let (backend, active, replica_read) = {
            let session = self
                .session
                .lock()
                .map_err(|_| Error::backend("session lock poisoned"))?;
            (
                Arc::clone(&session.backend),
                session.active_transaction.clone(),
                session.replica_read,
            )
        };
        // 有效活跃事务直接用其快照；否则按固定 ts 新建。
        let mut snapshot = match active.filter(|transaction| transaction.valid) {
            Some(transaction) => transaction.snapshot,
            None => {
                let mut snapshot = backend.snapshot_with_ts(self.ts)?;
                snapshot.temporary_table_interceptor = self
                    .info_schema
                    .as_ref()
                    .is_some_and(|info| info.local_temporary_tables_attached);
                snapshot
            }
        };
        // Follower/Mixed 副本读偏好透传到快照。
        if replica_read.is_follower_read() {
            snapshot.replica_read = replica_read;
        }
        snapshot.staleness_read_only = true;
        Ok(snapshot)
    }

    /// 过期读不支持 ForUpdate 快照。
    pub fn snapshot_with_stmt_for_update_ts(&self) -> Result<Snapshot, Error> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "GetSnapshotWithStmtForUpdateTS not supported for stalenessTxnProvider",
        ))
    }

    /// 本地临时表创建通知（过期读无额外处理）。
    pub fn on_local_temporary_table_created(&self) {}

    /// 提交前选项设置占位。
    pub fn set_options_before_commit(
        &self,
        _transaction: &mut Transaction,
        _commit_ts_checker: impl Fn(u64) -> bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

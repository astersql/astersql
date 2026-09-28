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

// 悲观可重复读（Repeatable Read，RR）隔离级别的事务上下文 Provider。
//
// 可重复读保证同一事务内普通读看到一致快照（读时间戳固定为 start ts），
// 而 FOR UPDATE / 加锁读可按需从时间戳预言机（Oracle/TSO）取更新的
// for_update_ts。本模块还根据执行计划形状决定是否可跳过向 PD 取最新 TSO。

use crate::{
    BasePessimisticTxnContextProvider, BaseTxnContextProvider, EnterNewTxnType, IsolationLevel,
    IsolationRuntime, PlanInspection, PlanKind, RuntimeContext, Snapshot, StatementInspection,
    StmtErrorAction, StmtErrorHandlePoint, TxnError, TxnErrorKind,
};

/// 悲观可重复读事务上下文 Provider。
pub struct PessimisticRRTxnContextProvider {
    /// 悲观事务共享基类（含 BaseTxnContextProvider）。
    pub base: BasePessimisticTxnContextProvider,
    /// 当前语句缓存的 FOR UPDATE 时间戳；0 表示尚未取到。
    pub for_update_ts: u64,
    /// 最近一次因错误刷新得到的 for_update_ts，供重试时沿用。
    pub latest_for_update_ts: u64,
    /// 为真时允许复用会话上已有的 for_update_ts，而不再向 PD 取最新 TSO。
    pub optimize_for_not_fetching_latest_ts: bool,
}

/// 构造悲观可重复读 Provider。
pub fn NewPessimisticRRTxnContextProvider(
    runtime: Box<dyn IsolationRuntime>,
    causal_consistency_only: bool,
) -> PessimisticRRTxnContextProvider {
    PessimisticRRTxnContextProvider {
        base: BasePessimisticTxnContextProvider {
            base: BaseTxnContextProvider::new(
                runtime,
                IsolationLevel::RepeatableRead,
                true,
                causal_consistency_only,
            ),
        },
        for_update_ts: 0,
        latest_for_update_ts: 0,
        optimize_for_not_fetching_latest_ts: false,
    }
}

impl PessimisticRRTxnContextProvider {
    /// 进入新事务时初始化（转发基类）。
    pub fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError> {
        self.base.base.OnInitialize(context, enter)
    }

    /// 取语句读时间戳：RR 下固定为事务 start ts。
    pub fn GetStmtReadTS(&mut self) -> Result<u64, TxnError> {
        self.base.base.GetStmtReadTS()
    }

    /// 取语句 FOR UPDATE 时间戳；若设置了 `tidb_snapshot` 则优先用快照 ts。
    pub fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError> {
        // 会话级历史快照覆盖实时事务时间戳
        if self.base.base.runtime.session().snapshot_ts != 0 {
            self.base.base.ActivateTxn()?;
            return Ok(self.base.base.runtime.session().snapshot_ts);
        }
        self.GetForUpdateTS()
    }

    /// 解析并缓存本语句的 for_update_ts（可走优化路径或向 Oracle 取新值）。
    pub fn GetForUpdateTS(&mut self) -> Result<u64, TxnError> {
        if self.for_update_ts != 0 {
            return Ok(self.for_update_ts);
        }
        self.base.base.ActivateTxn()?;
        // 计划优化：复用会话上已有的 for_update_ts，避免额外 TSO 往返
        if self.optimize_for_not_fetching_latest_ts {
            self.for_update_ts = self.base.base.runtime.session().txn.for_update_ts;
            return Ok(self.for_update_ts);
        }
        let session = self.base.base.runtime.session();
        let scope = session.txn.txn_scope.clone();
        let low_resolution = session.use_low_resolution_tso;
        let timestamp = self
            .base
            .base
            .runtime
            .oracle_future(&self.base.base.context, &scope, low_resolution)?
            .wait()?;
        self.base.base.runtime.session_mut().txn.for_update_ts = timestamp;
        self.base
            .base
            .runtime
            .set_transaction_snapshot_ts(timestamp)?;
        self.for_update_ts = timestamp;
        Ok(timestamp)
    }

    /// 从 Oracle 刷新最新 for_update_ts（加锁冲突重试前常用）。
    pub fn UpdateForUpdateTS(&mut self) -> Result<u64, TxnError> {
        if !self.base.base.txn_active {
            return Err(TxnError::new(
                TxnErrorKind::InvalidTransaction,
                "cannot refresh for-update timestamp on an inactive transaction",
            ));
        }
        let scope = self.base.base.runtime.session().txn.txn_scope.clone();
        let timestamp = self
            .base
            .base
            .runtime
            .latest_timestamp(&self.base.base.context, &scope)?;
        self.base.base.runtime.session_mut().txn.for_update_ts = timestamp;
        self.base
            .base
            .runtime
            .set_transaction_snapshot_ts(timestamp)?;
        self.latest_for_update_ts = timestamp;
        Ok(timestamp)
    }

    /// 语句开始：清空本语句 for_update_ts 缓存与计划优化标志。
    pub fn OnStmtStart(
        &mut self,
        context: RuntimeContext,
        statement: &dyn StatementInspection,
    ) -> Result<(), TxnError> {
        self.base.base.OnStmtStart(context, statement)?;
        self.for_update_ts = 0;
        self.optimize_for_not_fetching_latest_ts = false;
        Ok(())
    }

    /// 语句重试：尽量沿用上次刷新的 for_update_ts，并关闭计划优化。
    pub fn OnStmtRetry(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        self.base.base.OnStmtRetry(context)?;
        // 重试时优先携带已刷新的 latest_for_update_ts，避免读到过期锁视图
        self.for_update_ts = if self.latest_for_update_ts > self.for_update_ts {
            self.latest_for_update_ts
        } else {
            0
        };
        self.optimize_for_not_fetching_latest_ts = false;
        Ok(())
    }

    /// 仅在悲观加锁后切入点处理错误；其它点返回 NoIdea。
    pub fn OnStmtErrorForNextAction(
        &mut self,
        context: RuntimeContext,
        point: StmtErrorHandlePoint,
        error: TxnError,
    ) -> StmtErrorAction {
        self.base.base.context = context;
        if point != StmtErrorHandlePoint::AfterPessimisticLock {
            return StmtErrorAction::NoIdea;
        }
        self.HandleAfterPessimisticLockError(error)
    }

    /// 处理悲观加锁后错误：可重试冲突则刷新 for_update_ts 并准备重试。
    fn HandleAfterPessimisticLockError(&mut self, error: TxnError) -> StmtErrorAction {
        match error.kind {
            TxnErrorKind::Deadlock { retryable: false } => {
                return StmtErrorAction::Error(error);
            }
            TxnErrorKind::Deadlock { retryable: true } => {
                // 可重试死锁：先取消公平加锁（fair locking）状态
                if let Err(cancel_error) = self.base.CancelFairLockingIfNeeded() {
                    return StmtErrorAction::Error(cancel_error);
                }
            }
            TxnErrorKind::WriteConflict => {
                // 锁等待已超时则升级为 LockWaitTimeout，不再重试
                if self.base.base.runtime.session().lock_wait_elapsed_ms
                    >= self.base.base.runtime.session().lock_wait_timeout_ms
                {
                    return StmtErrorAction::Error(TxnError::new(
                        TxnErrorKind::LockWaitTimeout,
                        "lock wait timeout",
                    ));
                }
            }
            _ => {
                // 其它错误仍刷新 ts，避免后续语句卡在陈旧锁时间戳上
                let _ = self.UpdateForUpdateTS();
                return StmtErrorAction::Error(error);
            }
        }
        if self.UpdateForUpdateTS().is_err() {
            return StmtErrorAction::Error(error);
        }
        match self.base.RetryFairLockingIfNeeded() {
            Ok(()) => StmtErrorAction::RetryReady,
            Err(retry_error) => StmtErrorAction::Error(retry_error),
        }
    }

    /// 根据执行计划建议是否可跳过向 PD 取最新 TSO。
    pub fn AdviseOptimizeWithPlan(&mut self, plan: &dyn PlanInspection) {
        if self.base.base.IsSnapshotEnabled() || self.base.base.IsBeginStmtWithStaleRead() {
            return;
        }
        self.optimize_for_not_fetching_latest_ts = NotNeedGetLatestTSFromPD(plan, false);
    }

    /// 按语句读时间戳构造 RR 快照。
    pub fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtReadTS()?;
        self.base
            .base
            .GetSnapshotByTS(timestamp, IsolationLevel::RepeatableRead)
    }

    /// 按语句 FOR UPDATE 时间戳构造 RR 快照。
    pub fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtForUpdateTS()?;
        self.base
            .base
            .GetSnapshotByTS(timestamp, IsolationLevel::RepeatableRead)
    }
}

/// 判断执行计划是否无需向 PD 取最新 TSO。
///
/// 对单行加锁 PointGet / 无 SELECT 的 Insert 等可复用已有 for_update_ts；
/// 全表扫描等则必须重新取最新时间戳。
pub fn NotNeedGetLatestTSFromPD(plan: &dyn PlanInspection, in_lock_or_write_stmt: bool) -> bool {
    match plan.kind() {
        PlanKind::PointGet { lock, .. } | PlanKind::BatchPointGet { lock } => {
            !in_lock_or_write_stmt || lock
        }
        PlanKind::Physical { lock } => {
            let children = plan.children();
            !children.is_empty()
                && children
                    .iter()
                    .all(|child| NotNeedGetLatestTSFromPD(*child, lock || in_lock_or_write_stmt))
        }
        PlanKind::Update | PlanKind::Delete => plan
            .children()
            .first()
            .is_some_and(|child| NotNeedGetLatestTSFromPD(*child, true)),
        PlanKind::Insert { has_select, .. } => !has_select,
        PlanKind::Execute => plan
            .children()
            .first()
            .is_some_and(|child| NotNeedGetLatestTSFromPD(*child, in_lock_or_write_stmt)),
        PlanKind::Projection
        | PlanKind::PhysicalIndexReader { .. }
        | PlanKind::PhysicalTableReader { .. }
        | PlanKind::Other => false,
    }
}

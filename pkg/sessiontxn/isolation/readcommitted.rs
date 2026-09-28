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

// 悲观读已提交（Read Committed，RC）隔离级别的事务上下文 Provider。
//
// 读已提交允许每条语句看到最新已提交数据：语句时间戳可按语句刷新。
// 本模块实现 RC-check ts（只读语句复用/校验时间戳）、写冲突后重试，
// 以及根据执行计划跳过向 PD 取 TSO 的优化。

use crate::{
    BasePessimisticTxnContextProvider, BaseTxnContextProvider, ConstantFuture, EnterNewTxnType,
    IsolationLevel, IsolationRuntime, PlanInspection, PlanKind, RuntimeContext, Snapshot,
    StatementInspection, StmtErrorAction, StmtErrorHandlePoint, TimestampFuture, TxnError,
    TxnErrorKind,
};

/// 语句时间戳的来源：事务 start ts、常量缓存，或向 Oracle 异步取数。
enum StmtTimestampSource {
    /// 直接使用事务激活时的 start ts。
    StartTimestamp,
    /// 使用已缓存的常量时间戳（如 latest_oracle_ts）。
    Constant(u64),
    /// 通过时间戳预言机 Future 异步获取。
    Oracle(Box<dyn TimestampFuture>),
}

/// 单条语句的时间戳状态。
#[derive(Default)]
pub struct StmtState {
    /// 已解析出的语句时间戳；0 表示尚未取值。
    pub stmt_ts: u64,
    /// 时间戳来源；None 表示尚未 Prepare。
    stmt_ts_source: Option<StmtTimestampSource>,
    /// 为真时本语句应使用事务 start ts。
    pub stmt_use_start_ts: bool,
}

impl StmtState {
    /// 为新语句重置时间戳状态。
    fn PrepareStmt(&mut self, use_start_ts: bool) {
        *self = Self {
            stmt_ts: 0,
            stmt_ts_source: None,
            stmt_use_start_ts: use_start_ts,
        };
    }
}

/// 悲观读已提交事务上下文 Provider。
pub struct PessimisticRCTxnContextProvider {
    /// 悲观事务共享基类。
    pub base: BasePessimisticTxnContextProvider,
    /// 当前语句时间戳状态。
    pub stmt: StmtState,
    /// 最近一次从 Oracle 取到的时间戳（供 RC-check 复用）。
    pub latest_oracle_ts: u64,
    /// `latest_oracle_ts` 是否仍然有效（重试后会失效）。
    pub latest_oracle_ts_valid: bool,
    /// 写语句快照是否开启 RC check ts。
    pub check_ts_in_write_stmt: bool,
}

/// 构造悲观读已提交 Provider。
pub fn NewPessimisticRCTxnContextProvider(
    runtime: Box<dyn IsolationRuntime>,
    causal_consistency_only: bool,
) -> PessimisticRCTxnContextProvider {
    PessimisticRCTxnContextProvider {
        base: BasePessimisticTxnContextProvider {
            base: BaseTxnContextProvider::new(
                runtime,
                IsolationLevel::ReadCommitted,
                true,
                causal_consistency_only,
            ),
        },
        stmt: StmtState::default(),
        latest_oracle_ts: 0,
        latest_oracle_ts_valid: false,
        check_ts_in_write_stmt: false,
    }
}

impl PessimisticRCTxnContextProvider {
    /// 进入新事务：激活后用 start ts 初始化 latest_oracle_ts。
    pub fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError> {
        self.base.base.OnInitialize(context, enter)?;
        if self.base.base.txn_active {
            self.latest_oracle_ts = self.base.base.runtime.session().txn.start_ts;
            self.latest_oracle_ts_valid = true;
        }
        Ok(())
    }

    /// 激活事务；首次激活时同步刷新 latest_oracle_ts。
    pub fn ActivateTxn(&mut self) -> Result<u64, TxnError> {
        let was_active = self.base.base.txn_active;
        let timestamp = self.base.base.ActivateTxn()?;
        if !was_active {
            self.latest_oracle_ts = timestamp;
            self.latest_oracle_ts_valid = true;
        }
        Ok(timestamp)
    }

    /// 语句开始：设置 RC-check 标志并准备语句时间戳状态。
    pub fn OnStmtStart(
        &mut self,
        context: RuntimeContext,
        statement: &dyn StatementInspection,
    ) -> Result<(), TxnError> {
        self.base.base.OnStmtStart(context, statement)?;
        self.base.base.runtime.session_mut().stmt_rc_check_ts = false;
        if NeedSetRCCheckTSFlag(self.base.base.runtime.session(), statement) {
            self.base.base.runtime.session_mut().stmt_rc_check_ts = true;
        }
        self.check_ts_in_write_stmt = false;
        // 事务尚未 Prepare 时，首条语句可用 start ts
        self.stmt.PrepareStmt(!self.base.base.is_txn_prepared);
        Ok(())
    }

    /// 语句重试：使 latest_oracle_ts 失效，强制重新取数。
    pub fn OnStmtRetry(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        self.base.base.OnStmtRetry(context)?;
        self.latest_oracle_ts_valid = false;
        self.check_ts_in_write_stmt = false;
        self.stmt.PrepareStmt(false);
        Ok(())
    }

    /// 按策略选择语句时间戳来源（尚未选择时才执行）。
    fn PrepareStmtTS(&mut self) -> Result<(), TxnError> {
        if self.stmt.stmt_ts_source.is_some() {
            return Ok(());
        }
        self.stmt.stmt_ts_source = Some(if self.stmt.stmt_use_start_ts {
            StmtTimestampSource::StartTimestamp
        } else if self.latest_oracle_ts_valid && self.base.base.runtime.session().stmt_rc_check_ts {
            // RC-check：复用已缓存的 oracle ts，避免每条只读语句都向 PD 取 TSO
            StmtTimestampSource::Constant(self.latest_oracle_ts)
        } else {
            let session = self.base.base.runtime.session();
            let scope = session.txn.txn_scope.clone();
            let low_resolution = session.use_low_resolution_tso;
            StmtTimestampSource::Oracle(self.base.base.runtime.oracle_future(
                &self.base.base.context,
                &scope,
                low_resolution,
            )?)
        });
        Ok(())
    }

    /// 解析并返回本语句时间戳，同时写回快照与 for_update_ts。
    pub fn GetStmtTS(&mut self) -> Result<u64, TxnError> {
        if self.stmt.stmt_ts != 0 {
            return Ok(self.stmt.stmt_ts);
        }
        let start_ts = self.ActivateTxn()?;
        // Unlike a stale `latest_oracle_ts_valid`, first-time activation
        // already primed `latest_oracle_ts` via `ActivateTxn` above (see the
        // `PessimisticRCTxnContextProvider::ActivateTxn` override, which
        // mirrors Go's `onTxnActiveFunc`). Do not resurrect an
        // `OnStmtRetry`-invalidated cache here: `PrepareStmtTS` must be
        // allowed to fall through to a fresh oracle fetch so a retried
        // statement observes a strictly newer read-committed snapshot,
        // exactly like Go's `getStmtTS`/`prepareStmtTS`.
        // 首次 Activate 已写入 latest_oracle_ts；但 OnStmtRetry 会使缓存失效，
        // 必须允许 PrepareStmtTS 走新的 Oracle 取数，以便重试看到更新的 RC 快照。
        self.PrepareStmtTS()?;
        let timestamp = match self.stmt.stmt_ts_source.as_mut().unwrap() {
            StmtTimestampSource::StartTimestamp => start_ts,
            StmtTimestampSource::Constant(timestamp) => *timestamp,
            StmtTimestampSource::Oracle(future) => {
                let timestamp = future.wait()?;
                self.latest_oracle_ts = timestamp;
                self.latest_oracle_ts_valid = true;
                timestamp
            }
        };
        self.base
            .base
            .runtime
            .set_transaction_snapshot_ts(timestamp)?;
        self.base.base.runtime.session_mut().txn.for_update_ts = timestamp;
        self.stmt.stmt_ts = timestamp;
        Ok(timestamp)
    }

    /// 取语句读时间戳；`tidb_snapshot` 非 0 时优先返回快照 ts。
    pub fn GetStmtReadTS(&mut self) -> Result<u64, TxnError> {
        if self.base.base.runtime.session().snapshot_ts != 0 {
            self.base.base.ActivateTxn()?;
            Ok(self.base.base.runtime.session().snapshot_ts)
        } else {
            self.GetStmtTS()
        }
    }

    /// RC 下 FOR UPDATE 时间戳与读时间戳相同。
    pub fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError> {
        self.GetStmtReadTS()
    }

    /// 按错误切入点分派处理（查询后 / 悲观加锁后）。
    pub fn OnStmtErrorForNextAction(
        &mut self,
        context: RuntimeContext,
        point: StmtErrorHandlePoint,
        error: TxnError,
    ) -> StmtErrorAction {
        self.base.base.context = context;
        match point {
            StmtErrorHandlePoint::AfterQuery => self.HandleAfterQueryError(error),
            StmtErrorHandlePoint::AfterPessimisticLock => {
                self.HandleAfterPessimisticLockError(error)
            }
            _ => self.base.base.OnStmtErrorForNextAction(point, error),
        }
    }

    /// 查询后写冲突且开启 RC-check 时可重试。
    fn HandleAfterQueryError(&self, error: TxnError) -> StmtErrorAction {
        if error.kind == TxnErrorKind::WriteConflict
            && self.base.base.runtime.session().stmt_rc_check_ts
        {
            StmtErrorAction::RetryReady
        } else {
            StmtErrorAction::NoIdea
        }
    }

    /// 悲观加锁后：可重试死锁/写冲突则准备重试，锁等待超时则直接报错。
    fn HandleAfterPessimisticLockError(&mut self, error: TxnError) -> StmtErrorAction {
        let retryable = match error.kind {
            TxnErrorKind::Deadlock { retryable: true } => {
                if let Err(cancel_error) = self.base.CancelFairLockingIfNeeded() {
                    return StmtErrorAction::Error(cancel_error);
                }
                true
            }
            TxnErrorKind::WriteConflict => {
                if self.base.base.runtime.session().lock_wait_elapsed_ms
                    >= self.base.base.runtime.session().lock_wait_timeout_ms
                {
                    return StmtErrorAction::Error(TxnError::new(
                        TxnErrorKind::LockWaitTimeout,
                        "lock wait timeout",
                    ));
                }
                true
            }
            _ => false,
        };
        if retryable {
            match self.base.RetryFairLockingIfNeeded() {
                Ok(()) => StmtErrorAction::RetryReady,
                Err(retry_error) => StmtErrorAction::Error(retry_error),
            }
        } else {
            StmtErrorAction::Error(error)
        }
    }

    /// 预热：Prepare 事务并在非快照模式下预先选择语句时间戳来源。
    pub fn AdviseWarmup(&mut self) -> Result<(), TxnError> {
        self.base.base.PrepareTxn()?;
        if !self.base.base.IsSnapshotEnabled() {
            self.PrepareStmtTS()?;
        }
        Ok(())
    }

    /// 根据执行计划决定写路径是否跳过向 PD 取 TSO，并固定常量时间戳来源。
    pub fn AdviseOptimizeWithPlan(&mut self, plan: &dyn PlanInspection) {
        if self.base.base.IsSnapshotEnabled()
            || self.base.base.IsBeginStmtWithStaleRead()
            || self.stmt.stmt_use_start_ts
            || !self.latest_oracle_ts_valid
            || self.base.base.runtime.session().retrying
        {
            return;
        }
        if PlanSkipGetTSOFromPD(
            self.base.base.runtime.session().rc_write_check_ts,
            plan,
            false,
        ) {
            self.check_ts_in_write_stmt = true;
            self.stmt.stmt_ts_source = Some(StmtTimestampSource::Constant(self.latest_oracle_ts));
        }
    }

    /// 按 FOR UPDATE 时间戳构造 RC 快照，并带上写语句 RC-check 标志。
    pub fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtForUpdateTS()?;
        let mut snapshot = self
            .base
            .base
            .GetSnapshotByTS(timestamp, IsolationLevel::ReadCommitted)?;
        snapshot.rc_check_ts = self.check_ts_in_write_stmt;
        Ok(snapshot)
    }

    /// 按读时间戳构造 RC 快照，并带上会话 stmt_rc_check_ts 标志。
    pub fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtReadTS()?;
        let mut snapshot = self
            .base
            .base
            .GetSnapshotByTS(timestamp, IsolationLevel::ReadCommitted)?;
        snapshot.rc_check_ts = self.base.base.runtime.session().stmt_rc_check_ts;
        Ok(snapshot)
    }
}

/// 是否应为当前只读语句设置 RC-check ts 标志。
///
/// 需要已建立连接、开启 rc_read_check_ts、处于显式事务且非重试中的只读语句。
pub fn NeedSetRCCheckTSFlag(
    session: &crate::SessionState,
    statement: &dyn StatementInspection,
) -> bool {
    session.connection_id > 0
        && session.rc_read_check_ts_enabled
        && session.in_txn
        && !session.retrying
        && statement.is_read_only()
}

/// 判断执行计划是否可跳过向 PD 取 TSO（RC 写路径 check-ts 优化）。
pub fn PlanSkipGetTSOFromPD(
    rc_write_check_ts: bool,
    plan: &dyn PlanInspection,
    in_lock_or_write_stmt: bool,
) -> bool {
    match plan.kind() {
        PlanKind::PointGet { lock, .. } => rc_write_check_ts && (lock || in_lock_or_write_stmt),
        PlanKind::Physical { lock } => {
            let children = plan.children();
            !children.is_empty()
                && children.iter().all(|child| {
                    PlanSkipGetTSOFromPD(rc_write_check_ts, *child, lock || in_lock_or_write_stmt)
                })
        }
        PlanKind::Update | PlanKind::Delete => plan
            .children()
            .first()
            .is_some_and(|child| PlanSkipGetTSOFromPD(rc_write_check_ts, *child, true)),
        // 纯 VALUES Insert（无 SELECT / ON DUPLICATE / REPLACE）可跳过
        PlanKind::Insert {
            has_select,
            on_duplicate,
            replace,
        } => !has_select && !on_duplicate && !replace,
        PlanKind::Execute => plan.children().first().is_some_and(|child| {
            PlanSkipGetTSOFromPD(rc_write_check_ts, *child, in_lock_or_write_stmt)
        }),
        PlanKind::BatchPointGet { .. }
        | PlanKind::Projection
        | PlanKind::PhysicalIndexReader { .. }
        | PlanKind::PhysicalTableReader { .. }
        | PlanKind::Other => false,
    }
}

/// 构造返回固定时间戳的 Future（测试/占位用）。
#[allow(dead_code)]
fn constant_future(timestamp: u64) -> Box<dyn TimestampFuture> {
    Box::new(ConstantFuture(timestamp))
}

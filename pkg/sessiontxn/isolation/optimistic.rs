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

// 乐观事务（Optimistic）隔离上下文提供者。
//
// 在自动提交的 PointGet（点查）场景下可用 `MAX_TIMESTAMP` 优化读时间戳；
// 并维护乐观事务是否可重试（could_retry）标志。对应 Go `optimistic.go`。

use crate::{
    BaseTxnContextProvider, EnterNewTxnType, IsolationLevel, IsolationRuntime, MAX_TIMESTAMP,
    PlanInspection, PlanKind, RuntimeContext, Snapshot, TxnError,
};

/// 乐观事务上下文提供者：包装 `BaseTxnContextProvider`，并跟踪 max-ts 优化开关。
pub struct OptimisticTxnContextProvider {
    /// 共享的事务生命周期与快照逻辑基类。
    pub base: BaseTxnContextProvider,
    /// 为 true 时语句读 TS 直接返回 `MAX_TIMESTAMP`（点查优化已生效）。
    pub optimize_with_max_ts: bool,
}

/// 构造乐观隔离、非悲观（pessimistic=false）的提供者。
pub fn NewOptimisticTxnContextProvider(
    runtime: Box<dyn IsolationRuntime>,
    causal_consistency_only: bool,
) -> OptimisticTxnContextProvider {
    OptimisticTxnContextProvider {
        base: BaseTxnContextProvider::new(
            runtime,
            IsolationLevel::Optimistic,
            false,
            causal_consistency_only,
        ),
        optimize_with_max_ts: false,
    }
}

impl OptimisticTxnContextProvider {
    /// 初始化：清除 max-ts 优化，委托基类，再刷新可重试标志。
    pub fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError> {
        self.optimize_with_max_ts = false;
        self.base.OnInitialize(context, enter)?;
        self.RefreshRetryable();
        Ok(())
    }

    /// 根据会话状态与进入方式更新 `txn.could_retry`。
    pub fn RefreshRetryable(&mut self) {
        let retryable =
            IsOptimisticTxnRetryable(self.base.runtime.session(), self.base.enter_new_txn_type);
        self.base.runtime.session_mut().txn.could_retry = retryable;
    }

    /// 激活事务后刷新可重试标志。
    pub fn ActivateTxn(&mut self) -> Result<u64, TxnError> {
        let timestamp = self.base.ActivateTxn()?;
        self.RefreshRetryable();
        Ok(timestamp)
    }

    /// 语句读时间戳：启用 max-ts 优化时返回 `MAX_TIMESTAMP`，否则走基类。
    pub fn GetStmtReadTS(&mut self) -> Result<u64, TxnError> {
        if self.optimize_with_max_ts {
            Ok(MAX_TIMESTAMP)
        } else {
            self.base.GetStmtReadTS()
        }
    }

    /// 乐观模式下 for-update TS 与读 TS 相同。
    pub fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError> {
        self.GetStmtReadTS()
    }

    /// 按语句读 TS 构造乐观隔离快照。
    pub fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtReadTS()?;
        self.base
            .GetSnapshotByTS(timestamp, IsolationLevel::Optimistic)
    }

    /// for-update 快照与读快照相同。
    pub fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError> {
        self.GetSnapshotWithStmtReadTS()
    }

    /// 若计划是自动提交点查候选，则强制用 `MAX_TIMESTAMP` 作为 start TS 并开启优化。
    pub fn AdviseOptimizeWithPlan(&mut self, plan: &dyn PlanInspection) -> Result<(), TxnError> {
        let session = self.base.runtime.session();
        // 已优化、已有快照/过期读、事务已激活、非 autocommit 或显式事务时跳过。
        if self.optimize_with_max_ts
            || self.base.IsSnapshotEnabled()
            || self.base.IsBeginStmtWithStaleRead()
            || self.base.txn_active
            || !session.autocommit
            || session.in_txn
        {
            return Ok(());
        }
        if IsAutoCommitPointGet(plan) {
            self.base.ForcePrepareConstStartTS(MAX_TIMESTAMP)?;
            self.optimize_with_max_ts = true;
        }
        Ok(())
    }
}

/// 判断计划树是否为自动提交场景下的点查（含 Projection / Execute 包装）。
fn IsAutoCommitPointGet(plan: &dyn PlanInspection) -> bool {
    match plan.kind() {
        PlanKind::Projection => plan
            .children()
            .first()
            .is_some_and(|child| IsPointGetCandidate(*child)),
        PlanKind::Execute => plan
            .children()
            .first()
            .is_some_and(|child| match child.kind() {
                PlanKind::Projection => child
                    .children()
                    .first()
                    .is_some_and(|projected| IsPointGetCandidate(*projected)),
                _ => IsPointGetCandidate(*child),
            }),
        _ => IsPointGetCandidate(plan),
    }
}

/// 点查候选：唯一索引点读、主键点读，或无二次读且非缓存表的 PointGet。
fn IsPointGetCandidate(plan: &dyn PlanInspection) -> bool {
    match plan.kind() {
        PlanKind::PhysicalIndexReader { unique_point_get } => unique_point_get,
        PlanKind::PhysicalTableReader {
            primary_key_point_get,
        } => primary_key_point_get,
        PlanKind::PointGet {
            no_second_read,
            cache_table,
            ..
        } => no_second_read && !cache_table,
        _ => false,
    }
}

/// 乐观事务是否允许自动重试：排除 Default 进入、流水线、零重试上限、已有 snapshot_ts 等。
pub fn IsOptimisticTxnRetryable(session: &crate::SessionState, enter: EnterNewTxnType) -> bool {
    if enter == EnterNewTxnType::Default
        || session.pipelined
        || session.retry_limit == 0
        || session.snapshot_ts != 0
    {
        return false;
    }
    !session.in_txn || session.restricted_sql || !session.disable_txn_auto_retry
}

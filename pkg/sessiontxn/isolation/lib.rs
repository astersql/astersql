// Copyright 2026 AsterSQL.

// 会话事务隔离级别（isolation）子 crate。
//
// 按 Optimistic / ReadCommitted / RepeatableRead / Serializable 提供
// `TxnContextProvider` 实现，并由 `RegisteredTxnContextProvider` 统一分发。
// 对应 Go `pkg/sessiontxn/isolation`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

mod base;
mod optimistic;
mod readcommitted;
mod repeatable_read;
mod serializable;

pub use base::*;
pub use optimistic::*;
pub use readcommitted::*;
pub use repeatable_read::*;
pub use serializable::*;

/// 注册表用的提供者种类（乐观 / 悲观 RC / 悲观 RR / 悲观 Serializable）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderKind {
    Optimistic,
    PessimisticReadCommitted,
    PessimisticRepeatableRead,
    PessimisticSerializable,
}

/// 事务上下文提供者接口：初始化、语句生命周期、读/写 TS、快照与提交选项。
pub trait TxnContextProvider {
    fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError>;
    fn GetTxnInfoSchema(&mut self) -> TxnInfoSchemaRef;
    fn GetTxnScope(&self) -> String;
    fn GetReadReplicaScope(&self) -> String;
    fn OnStmtStart(
        &mut self,
        context: RuntimeContext,
        statement: &dyn StatementInspection,
    ) -> Result<(), TxnError>;
    fn OnPessimisticStmtStart(&mut self, context: RuntimeContext) -> Result<(), TxnError>;
    fn OnPessimisticStmtEnd(
        &mut self,
        context: RuntimeContext,
        successful: bool,
    ) -> Result<(), TxnError>;
    fn OnStmtRetry(&mut self, context: RuntimeContext) -> Result<(), TxnError>;
    fn OnStmtCommit(&mut self, context: RuntimeContext) -> Result<(), TxnError>;
    fn OnStmtRollback(
        &mut self,
        context: RuntimeContext,
        pessimistic_retry: bool,
    ) -> Result<(), TxnError>;
    fn OnStmtErrorForNextAction(
        &mut self,
        context: RuntimeContext,
        point: StmtErrorHandlePoint,
        error: TxnError,
    ) -> StmtErrorAction;
    fn GetStmtReadTS(&mut self) -> Result<u64, TxnError>;
    fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError>;
    fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError>;
    fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError>;
    fn ActivateTxn(&mut self) -> Result<u64, TxnError>;
    fn AdviseWarmup(&mut self) -> Result<(), TxnError>;
    fn AdviseOptimizeWithPlan(&mut self, plan: &dyn PlanInspection) -> Result<(), TxnError>;
    fn OnLocalTemporaryTableCreated(&mut self);
    fn SetOptionsBeforeCommit(
        &mut self,
        has_commit_ts_checker: bool,
    ) -> Result<CommitOptions, TxnError>;
}

/// 四种具体提供者的枚举包装，便于按 `ProviderKind` 统一持有。
pub enum RegisteredTxnContextProvider {
    Optimistic(OptimisticTxnContextProvider),
    PessimisticReadCommitted(PessimisticRCTxnContextProvider),
    PessimisticRepeatableRead(PessimisticRRTxnContextProvider),
    PessimisticSerializable(PessimisticSerializableTxnContextProvider),
}

/// 按种类构造注册表提供者。
pub fn NewRegisteredTxnContextProvider(
    kind: ProviderKind,
    runtime: Box<dyn IsolationRuntime>,
    causal_consistency_only: bool,
) -> RegisteredTxnContextProvider {
    match kind {
        ProviderKind::Optimistic => RegisteredTxnContextProvider::Optimistic(
            NewOptimisticTxnContextProvider(runtime, causal_consistency_only),
        ),
        ProviderKind::PessimisticReadCommitted => {
            RegisteredTxnContextProvider::PessimisticReadCommitted(
                NewPessimisticRCTxnContextProvider(runtime, causal_consistency_only),
            )
        }
        ProviderKind::PessimisticRepeatableRead => {
            RegisteredTxnContextProvider::PessimisticRepeatableRead(
                NewPessimisticRRTxnContextProvider(runtime, causal_consistency_only),
            )
        }
        ProviderKind::PessimisticSerializable => {
            RegisteredTxnContextProvider::PessimisticSerializable(
                NewPessimisticSerializableTxnContextProvider(runtime, causal_consistency_only),
            )
        }
    }
}

impl RegisteredTxnContextProvider {
    /// 取得可变的基类 `BaseTxnContextProvider`（RC/RR 多一层 `base.base`）。
    fn Base(&mut self) -> &mut BaseTxnContextProvider {
        match self {
            Self::Optimistic(provider) => &mut provider.base,
            Self::PessimisticReadCommitted(provider) => &mut provider.base.base,
            Self::PessimisticRepeatableRead(provider) => &mut provider.base.base,
            Self::PessimisticSerializable(provider) => &mut provider.base,
        }
    }

    /// 取得不可变的基类引用。
    fn BaseRef(&self) -> &BaseTxnContextProvider {
        match self {
            Self::Optimistic(provider) => &provider.base,
            Self::PessimisticReadCommitted(provider) => &provider.base.base,
            Self::PessimisticRepeatableRead(provider) => &provider.base.base,
            Self::PessimisticSerializable(provider) => &provider.base,
        }
    }
}

impl TxnContextProvider for RegisteredTxnContextProvider {
    fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError> {
        match self {
            Self::Optimistic(provider) => provider.OnInitialize(context, enter),
            Self::PessimisticReadCommitted(provider) => provider.OnInitialize(context, enter),
            Self::PessimisticRepeatableRead(provider) => provider.OnInitialize(context, enter),
            Self::PessimisticSerializable(provider) => provider.OnInitialize(context, enter),
        }
    }

    fn GetTxnInfoSchema(&mut self) -> TxnInfoSchemaRef {
        self.Base().GetTxnInfoSchema()
    }

    fn GetTxnScope(&self) -> String {
        self.BaseRef().GetTxnScope()
    }

    fn GetReadReplicaScope(&self) -> String {
        self.BaseRef().GetReadReplicaScope()
    }

    fn OnStmtStart(
        &mut self,
        context: RuntimeContext,
        statement: &dyn StatementInspection,
    ) -> Result<(), TxnError> {
        // RC/RR 有自定义语句开始逻辑（例如刷新 for_update_ts）。
        match self {
            Self::PessimisticReadCommitted(provider) => provider.OnStmtStart(context, statement),
            Self::PessimisticRepeatableRead(provider) => provider.OnStmtStart(context, statement),
            _ => self.Base().OnStmtStart(context, statement),
        }
    }

    fn OnPessimisticStmtStart(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        match self {
            Self::PessimisticReadCommitted(provider) => {
                provider.base.OnPessimisticStmtStart(context)
            }
            Self::PessimisticRepeatableRead(provider) => {
                provider.base.OnPessimisticStmtStart(context)
            }
            _ => Ok(()),
        }
    }

    fn OnPessimisticStmtEnd(
        &mut self,
        context: RuntimeContext,
        successful: bool,
    ) -> Result<(), TxnError> {
        match self {
            Self::PessimisticReadCommitted(provider) => {
                provider.base.OnPessimisticStmtEnd(context, successful)
            }
            Self::PessimisticRepeatableRead(provider) => {
                provider.base.OnPessimisticStmtEnd(context, successful)
            }
            _ => Ok(()),
        }
    }

    fn OnStmtRetry(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        match self {
            Self::PessimisticReadCommitted(provider) => provider.OnStmtRetry(context),
            Self::PessimisticRepeatableRead(provider) => provider.OnStmtRetry(context),
            _ => self.Base().OnStmtRetry(context),
        }
    }

    fn OnStmtCommit(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        self.Base().OnStmtCommit(context)
    }

    fn OnStmtRollback(
        &mut self,
        context: RuntimeContext,
        pessimistic_retry: bool,
    ) -> Result<(), TxnError> {
        self.Base().OnStmtRollback(context, pessimistic_retry)
    }

    fn OnStmtErrorForNextAction(
        &mut self,
        context: RuntimeContext,
        point: StmtErrorHandlePoint,
        error: TxnError,
    ) -> StmtErrorAction {
        match self {
            Self::PessimisticReadCommitted(provider) => {
                provider.OnStmtErrorForNextAction(context, point, error)
            }
            Self::PessimisticRepeatableRead(provider) => {
                provider.OnStmtErrorForNextAction(context, point, error)
            }
            Self::PessimisticSerializable(provider) => {
                provider.base.context = context;
                provider.OnStmtErrorForNextAction(point, error)
            }
            Self::Optimistic(provider) => {
                provider.base.context = context;
                provider.base.OnStmtErrorForNextAction(point, error)
            }
        }
    }

    fn GetStmtReadTS(&mut self) -> Result<u64, TxnError> {
        match self {
            Self::Optimistic(provider) => provider.GetStmtReadTS(),
            Self::PessimisticReadCommitted(provider) => provider.GetStmtReadTS(),
            Self::PessimisticRepeatableRead(provider) => provider.GetStmtReadTS(),
            Self::PessimisticSerializable(provider) => provider.GetStmtReadTS(),
        }
    }

    fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError> {
        match self {
            Self::Optimistic(provider) => provider.GetStmtForUpdateTS(),
            Self::PessimisticReadCommitted(provider) => provider.GetStmtForUpdateTS(),
            Self::PessimisticRepeatableRead(provider) => provider.GetStmtForUpdateTS(),
            Self::PessimisticSerializable(provider) => provider.GetStmtForUpdateTS(),
        }
    }

    fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError> {
        match self {
            Self::Optimistic(provider) => provider.GetSnapshotWithStmtReadTS(),
            Self::PessimisticReadCommitted(provider) => provider.GetSnapshotWithStmtReadTS(),
            Self::PessimisticRepeatableRead(provider) => provider.GetSnapshotWithStmtReadTS(),
            Self::PessimisticSerializable(provider) => provider.GetSnapshotWithStmtReadTS(),
        }
    }

    fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError> {
        match self {
            Self::Optimistic(provider) => provider.GetSnapshotWithStmtForUpdateTS(),
            Self::PessimisticReadCommitted(provider) => provider.GetSnapshotWithStmtForUpdateTS(),
            Self::PessimisticRepeatableRead(provider) => provider.GetSnapshotWithStmtForUpdateTS(),
            Self::PessimisticSerializable(provider) => provider.GetSnapshotWithStmtForUpdateTS(),
        }
    }

    fn ActivateTxn(&mut self) -> Result<u64, TxnError> {
        match self {
            Self::Optimistic(provider) => provider.ActivateTxn(),
            Self::PessimisticReadCommitted(provider) => provider.ActivateTxn(),
            Self::PessimisticRepeatableRead(provider) => provider.base.base.ActivateTxn(),
            Self::PessimisticSerializable(provider) => provider.base.ActivateTxn(),
        }
    }

    fn AdviseWarmup(&mut self) -> Result<(), TxnError> {
        match self {
            Self::PessimisticReadCommitted(provider) => provider.AdviseWarmup(),
            _ => self.Base().AdviseWarmup(),
        }
    }

    fn AdviseOptimizeWithPlan(&mut self, plan: &dyn PlanInspection) -> Result<(), TxnError> {
        match self {
            Self::Optimistic(provider) => provider.AdviseOptimizeWithPlan(plan),
            Self::PessimisticReadCommitted(provider) => {
                provider.AdviseOptimizeWithPlan(plan);
                Ok(())
            }
            Self::PessimisticRepeatableRead(provider) => {
                provider.AdviseOptimizeWithPlan(plan);
                Ok(())
            }
            Self::PessimisticSerializable(_) => Ok(()),
        }
    }

    fn OnLocalTemporaryTableCreated(&mut self) {
        self.Base().OnLocalTemporaryTableCreated();
    }

    fn SetOptionsBeforeCommit(
        &mut self,
        has_commit_ts_checker: bool,
    ) -> Result<CommitOptions, TxnError> {
        self.Base().SetOptionsBeforeCommit(has_commit_ts_checker)
    }
}

/// 隔离级别综合单元测试（MockRuntime + 各 Provider）。
#[cfg(test)]
mod base_test;
#[cfg(test)]
mod isolation_aster_unit_test;

/// 共享测试夹具（MockRuntime、TxnAssert 等）。
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod optimistic_test;
#[cfg(test)]
mod readcommitted_test;
#[cfg(test)]
mod repeatable_read_test;
#[cfg(test)]
mod serializable_test;

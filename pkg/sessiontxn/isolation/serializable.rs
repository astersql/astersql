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

// 悲观可串行化（Serializable）隔离级别的事务上下文 Provider。
//
// 可串行化要求事务调度结果等价于某种串行执行顺序。本 Provider 将读时间戳
// （read ts）与 FOR UPDATE 时间戳均固定为事务 start ts，且悲观加锁错误一律
// 向上返回、不自动重试。

use crate::{
    BaseTxnContextProvider, EnterNewTxnType, IsolationLevel, IsolationRuntime, RuntimeContext,
    Snapshot, StmtErrorAction, StmtErrorHandlePoint, TxnError,
};

/// 悲观可串行化事务上下文 Provider：在基类之上固定隔离级别并定制错误策略。
pub struct PessimisticSerializableTxnContextProvider {
    /// 共享的基础事务上下文（时间戳、快照、语句生命周期等）。
    pub base: BaseTxnContextProvider,
}

/// 构造悲观可串行化 Provider。
///
/// `causal_consistency_only` 为真时仅要求因果一致（不强制全局线性一致）。
pub fn NewPessimisticSerializableTxnContextProvider(
    runtime: Box<dyn IsolationRuntime>,
    causal_consistency_only: bool,
) -> PessimisticSerializableTxnContextProvider {
    PessimisticSerializableTxnContextProvider {
        base: BaseTxnContextProvider::new(
            runtime,
            IsolationLevel::Serializable,
            true,
            causal_consistency_only,
        ),
    }
}

impl PessimisticSerializableTxnContextProvider {
    /// 进入新事务时初始化（转发基类）。
    pub fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError> {
        self.base.OnInitialize(context, enter)
    }

    /// 取当前语句读时间戳（可串行化下等于 start ts）。
    pub fn GetStmtReadTS(&mut self) -> Result<u64, TxnError> {
        self.base.GetStmtReadTS()
    }

    /// 取当前语句 FOR UPDATE 时间戳（与读时间戳相同）。
    pub fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError> {
        self.base.GetStmtForUpdateTS()
    }

    /// 按语句读时间戳构造快照（Snapshot：指定时间戳下的一致性读视图）。
    pub fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtReadTS()?;
        self.base
            .GetSnapshotByTS(timestamp, IsolationLevel::Serializable)
    }

    /// 按 FOR UPDATE 时间戳取快照（可串行化下与读快照相同）。
    pub fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError> {
        self.GetSnapshotWithStmtReadTS()
    }

    /// 根据错误切入点决定下一步动作：加锁后错误一律返回，其它点无建议。
    pub fn OnStmtErrorForNextAction(
        &self,
        point: StmtErrorHandlePoint,
        error: TxnError,
    ) -> StmtErrorAction {
        match point {
            // 悲观加锁失败：可串行化不重试，直接暴露错误
            StmtErrorHandlePoint::AfterPessimisticLock => StmtErrorAction::Error(error),
            _ => StmtErrorAction::NoIdea,
        }
    }
}

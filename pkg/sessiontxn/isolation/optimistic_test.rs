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

// Rust port of `optimistic_test.go`. Every scenario below exercises the
// real `OptimisticTxnContextProvider` state machine through the
// `MockRuntime` harness declared in `main_test.rs`; see that file for why
// a mock runtime substitutes for the Go `testkit`/`mockstore` stack.

//
// 乐观事务上下文 Provider 的单元测试。
//
// 对应 Go `optimistic_test.go`：通过 `MockRuntime` 驱动真实的
// `OptimisticTxnContextProvider` 状态机，覆盖时间戳稳定性、
// 自动提交 PointGet 使用 MAX_TIMESTAMP、错误处理与 `tidb_snapshot` 覆盖。
use crate::main_test::*;
use crate::*;
use std::rc::Rc;

/// 构造无锁、无二次读的 PointGet 测试计划（用于 MAX_TIMESTAMP 优化场景）。
fn point_get_plan() -> TestPlan {
    TestPlan::leaf(PlanKind::PointGet {
        lock: false,
        no_second_read: true,
        cache_table: false,
    })
}

/// 对应 `TestOptimisticTxnContextProviderTS`：乐观 Provider 在同一事务内
/// 对所有语句返回相同 start ts；自动提交 PointGet 可切到 `MAX_TIMESTAMP`；
/// 一旦进入显式事务或关闭 autocommit 则不再使用该优化。
/// Mirrors `TestOptimisticTxnContextProviderTS`: the optimistic provider
/// keeps handing out the same start ts to every statement in a transaction,
/// switches to `MAX_TIMESTAMP` for an autocommit point-get plan, and stops
/// doing that as soon as the transaction is explicit or autocommit is off.
#[test]
fn optimistic_ts_is_stable_across_statements_and_maxes_out_for_autocommit_point_get() {
    // Explicit `begin optimistic`: ts should be stable across statements
    // and strictly newer than the pre-txn oracle baseline. The queue holds
    // the pre-txn baseline (50) plus the ts the activation itself consumes
    // from the oracle (999).
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[50, 999]);
    let clock = runtime.oracle_clock();
    let compare_ts = get_oracle_ts(&clock);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    let update_ts = provider.GetStmtForUpdateTS().unwrap();
    assert_eq!(read_ts, update_ts);
    assert!(read_ts > compare_ts);

    // For optimistic mode, ts should be the same for all statements.
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), read_ts);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);

    // When the plan is an autocommit point-get, `MAX_TIMESTAMP` is used
    // instead of ever asking the oracle for a real ts.
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[]);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    provider.AdviseOptimizeWithPlan(&point_get_plan()).unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), MAX_TIMESTAMP);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), MAX_TIMESTAMP);

    // If the oracle future was warmed up first, `MAX_TIMESTAMP` should
    // still win once the plan is advised (the const-ts override replaces
    // whatever future was already prepared).
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[77]);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    provider.base.AdviseWarmup().unwrap();
    provider.AdviseOptimizeWithPlan(&point_get_plan()).unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), MAX_TIMESTAMP);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), MAX_TIMESTAMP);

    // When it is in an explicit txn, we should not use `MAX_TIMESTAMP`
    // even for a point-get plan.
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[80, 999]);
    let clock = runtime.oracle_clock();
    let compare_ts = get_oracle_ts(&clock);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    provider.AdviseOptimizeWithPlan(&point_get_plan()).unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    let update_ts = provider.GetStmtForUpdateTS().unwrap();
    assert_eq!(read_ts, update_ts);
    assert!(read_ts > compare_ts);

    // When autocommit=0, we should not use `MAX_TIMESTAMP` either.
    let session = SessionState {
        autocommit: false,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[123, 999]);
    let clock = runtime.oracle_clock();
    let compare_ts = get_oracle_ts(&clock);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    provider.AdviseOptimizeWithPlan(&point_get_plan()).unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    let update_ts = provider.GetStmtForUpdateTS().unwrap();
    assert_eq!(read_ts, update_ts);
    assert!(read_ts > compare_ts);
}

/// 对应 `TestOptimisticHandleError`：仅 `AfterPessimisticLock` 会把错误
/// 向上抛出（乐观事务不在悲观加锁风格错误后重试），且任何错误路径都不推进时间戳。
/// Mirrors `TestOptimisticHandleError`: only `StmtErrAfterPessimisticLock`
/// ever surfaces the error (optimistic txns do not retry statements after
/// a pessimistic-lock-style error), and no error path ever moves the ts.
#[test]
fn optimistic_handle_error_only_errors_after_pessimistic_lock_point_and_never_moves_ts() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[10]);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    let start_ts = provider.base.runtime.session().txn.start_ts;

    let cases: Vec<(StmtErrorHandlePoint, TxnError)> = vec![
        (
            StmtErrorHandlePoint::AfterPessimisticLock,
            TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
        ),
        (
            StmtErrorHandlePoint::AfterPessimisticLock,
            TxnError::new(TxnErrorKind::Deadlock { retryable: true }, "deadlock"),
        ),
        (
            StmtErrorHandlePoint::AfterPessimisticLock,
            TxnError::new(TxnErrorKind::Deadlock { retryable: false }, "deadlock"),
        ),
        (
            StmtErrorHandlePoint::AfterPessimisticLock,
            TxnError::new(TxnErrorKind::InvalidTransaction, "test"),
        ),
        (
            StmtErrorHandlePoint::AfterQuery,
            TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
        ),
        (
            StmtErrorHandlePoint::AfterQuery,
            TxnError::new(TxnErrorKind::InvalidTransaction, "test"),
        ),
    ];

    for (point, error) in cases {
        provider
            .base
            .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
            .unwrap();
        let action = provider.base.OnStmtErrorForNextAction(point, error.clone());
        if point == StmtErrorHandlePoint::AfterPessimisticLock {
            assert_eq!(action, StmtErrorAction::Error(error));

            // next statement should not update ts
            provider
                .base
                .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
                .unwrap();
            assert_eq!(provider.GetStmtReadTS().unwrap(), start_ts);
            assert_eq!(provider.GetStmtForUpdateTS().unwrap(), start_ts);
        } else {
            assert_eq!(action, StmtErrorAction::NoIdea);

            // retry should not update ts
            provider
                .base
                .OnStmtRetry(RuntimeContext::default())
                .unwrap();
            assert_eq!(provider.GetStmtReadTS().unwrap(), start_ts);
            assert_eq!(provider.GetStmtForUpdateTS().unwrap(), start_ts);

            // OnStmtErrorForNextAction again
            provider
                .base
                .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
                .unwrap();
            let action = provider.base.OnStmtErrorForNextAction(point, error.clone());
            assert_eq!(action, StmtErrorAction::NoIdea);

            // next statement should not update ts
            provider
                .base
                .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
                .unwrap();
            assert_eq!(provider.GetStmtReadTS().unwrap(), start_ts);
            assert_eq!(provider.GetStmtForUpdateTS().unwrap(), start_ts);
        }
    }
}

/// 对应 `TestTidbSnapshotVarInOptimisticTxn`：设置 `tidb_snapshot` 时
/// Provider 必须按快照读而非实时事务读；清除变量后恢复事务内读。
/// Mirrors `TestTidbSnapshotVarInOptimisticTxn`: while `tidb_snapshot`
/// (`snapshot_ts`/`snapshot_info_schema`) is set, the provider must read
/// through the snapshot rather than the live txn, and switch back once the
/// snapshot variables are cleared.
#[test]
fn optimistic_prefers_tidb_snapshot_vars_over_the_live_txn() {
    let snapshot_schema: TxnInfoSchemaRef = Rc::new(MockInfoSchema {
        version: 7,
        extended: false,
    });
    let snapshot_ts = 42;
    let session = SessionState {
        snapshot_ts,
        snapshot_info_schema: Some(snapshot_schema),
        ..SessionState::default()
    };
    // `WithBeginStmt` explicitly refreshes the txn from the oracle (see
    // `PrepareTxnWithOracleTS`, which "does not consider snapshotTS"), so a
    // real ts (999) must still be queued even though `snapshot_ts` is set;
    // this mirrors the real begin executor's start ts exceeding the
    // `tidb_snapshot` timestamp in the Go test.
    let (runtime, _) = MockRuntime::new(session, &[999]);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    assert!(provider.base.runtime.session().txn.start_ts > snapshot_ts);

    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.base.GetTxnInfoSchema().schema_meta_version(), 7);
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, snapshot_ts);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);

    // Clearing `tidb_snapshot` restores normal txn-scoped reads.
    provider.base.runtime.session_mut().snapshot_ts = 0;
    provider.base.runtime.session_mut().snapshot_info_schema = None;
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let txn_start_ts = provider.base.runtime.session().txn.start_ts;
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_ne!(read_ts, snapshot_ts);
    assert_eq!(read_ts, txn_start_ts);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);
}

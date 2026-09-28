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

// Rust port of `readcommitted_test.go`, exercising the real
// `PessimisticRCTxnContextProvider` state machine through the
// `MockRuntime` harness declared in `main_test.rs`.
//
// `TestConflictErrorsInRC`, `TestFailedDMLConsistency1/2` and
// `TestRCProviderInitialize`'s `testfork` scope sweep are executor/DML and
// session-scope integration scenarios: they assert on actual SQL execution
// (`tk.MustExec`, `tk2.MustExec`, `admin check table`) and TiKV client
// scope injection (`tikvclient/injectTxnScope`), not on the
// `TxnContextProvider` state machine this crate owns. They stay out of
// scope for this package's harness the same way they are absent from
// `base.rs`/`readcommitted.rs`; the provider-level assertions those tests
// also make (rc-check ts churn, lock error handling, initialize/causal
// consistency semantics, `tidb_snapshot` overrides) are fully covered
// below against the real provider.

//
// 悲观读已提交（RC）事务上下文 Provider 的单元测试。
//
// 对应 Go `readcommitted_test.go`：通过 `MockRuntime` 覆盖 RC-check ts
// 复用/刷新、加锁错误重试、FOR UPDATE 始终取最新 ts、初始化语义与
// `tidb_snapshot` 覆盖。执行器/DML 集成场景不在本包范围内。
use crate::main_test::*;
use crate::*;

/// 构造开启 RC-check 的会话状态（显式事务内）。
fn rc_session(connection_id: u64) -> SessionState {
    SessionState {
        connection_id,
        in_txn: true,
        rc_read_check_ts_enabled: true,
        ..SessionState::default()
    }
}

/// 以 `WithBeginStmt` 初始化 RC Provider。
fn init_rc_provider(runtime: MockRuntime) -> PessimisticRCTxnContextProvider {
    let mut provider = NewPessimisticRCTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    provider
}

/// 对应 `TestPessimisticRCTxnContextProviderRCCheck`：RC-check ts 锚定在
/// 事务 start ts、跨只读语句复用，仅在 `OnStmtRetry` 失效后才从 Oracle 刷新；
/// 非写冲突错误不触发重试；非只读（FOR UPDATE）语句不享受 RC-check。
/// Mirrors `TestPessimisticRCTxnContextProviderRCCheck`: the RC-check ts is
/// pinned to the txn start ts, reused across read-only statements, and only
/// refreshed from the oracle once `OnStmtRetry` invalidates it. Errors other
/// than a write conflict after a query never trigger a retry, and a
/// for-update (non-read-only) statement never benefits from the RC check at
/// all.
#[test]
fn rc_check_ts_reuses_start_ts_and_refreshes_only_after_a_retry() {
    let session = rc_session(1);
    let (runtime, _) = MockRuntime::new(session, &[100, 150, 160, 170, 180, 190, 200]);
    let clock = runtime.oracle_clock();
    let mut provider = init_rc_provider(runtime);

    // First read-only statement: RC check ts equals the txn start ts.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let rc_check_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(rc_check_ts, 100);

    // Second statement reuses the same RC-check ts.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), rc_check_ts);

    // A statement that never calls GetStmtReadTS should not disturb the
    // next statement's reuse of the same RC-check ts either.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), rc_check_ts);

    // A write conflict after the query invalidates the RC check and is
    // retryable.
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterQuery,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady);
    let compare_ts = get_oracle_ts(&clock);
    assert!(compare_ts > rc_check_ts);
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    let rc_check_ts = provider.GetStmtReadTS().unwrap();
    assert!(rc_check_ts > compare_ts);
    assert_eq!(rc_check_ts, 160);

    // If the retry succeeds, the next statement still reuses the RC check.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), rc_check_ts);

    // A non-write-conflict error also disables the retry path (but does not
    // itself force a refresh): the next statement still reuses the ts.
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterQuery,
        TxnError::new(TxnErrorKind::InvalidTransaction, "err"),
    );
    assert_eq!(action, StmtErrorAction::NoIdea);
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), rc_check_ts);

    // `AfterPessimisticLock` still disables the RC check, and retrying
    // refreshes the ts again.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), rc_check_ts);
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady);
    let compare_ts = get_oracle_ts(&clock);
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    let rc_check_ts = provider.GetStmtReadTS().unwrap();
    assert!(rc_check_ts > compare_ts);
    assert_eq!(rc_check_ts, 180);
    let compare_ts = get_oracle_ts(&clock);
    assert!(compare_ts > rc_check_ts);

    // Only a read-only statement can benefit from the RC check: a
    // for-update statement always fetches a fresh ts and a write conflict
    // after its query is not retried.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let ts = provider.GetStmtReadTS().unwrap();
    assert!(ts > compare_ts);
    assert_eq!(ts, 200);
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterQuery,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::NoIdea);
}

/// 对应 `TestPessimisticRCTxnContextProviderLockError`：可重试锁错误
/// （写冲突、可重试死锁）变为 `RetryReady`；其它错误原样返回。
/// Mirrors `TestPessimisticRCTxnContextProviderLockError`: retryable lock
/// errors (write conflict, retryable deadlock) become `RetryReady`;
/// everything else propagates unchanged.
#[test]
fn rc_lock_error_retries_write_conflicts_and_retryable_deadlocks_only() {
    let (runtime, _) = MockRuntime::new(rc_session(1), &[100]);
    let mut provider = init_rc_provider(runtime);

    for error in [
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
        TxnError::new(TxnErrorKind::Deadlock { retryable: true }, "deadlock"),
    ] {
        provider
            .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
            .unwrap();
        let action = provider.OnStmtErrorForNextAction(
            RuntimeContext::default(),
            StmtErrorHandlePoint::AfterPessimisticLock,
            error,
        );
        assert_eq!(action, StmtErrorAction::RetryReady);
    }

    for error in [
        TxnError::new(TxnErrorKind::Deadlock { retryable: false }, "deadlock"),
        TxnError::new(TxnErrorKind::InvalidTransaction, "err"),
    ] {
        provider
            .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
            .unwrap();
        let action = provider.OnStmtErrorForNextAction(
            RuntimeContext::default(),
            StmtErrorHandlePoint::AfterPessimisticLock,
            error.clone(),
        );
        assert_eq!(action, StmtErrorAction::Error(error));
    }
}

/// 对应 `TestPessimisticRCTxnContextProviderTS`：FOR UPDATE 语句每次
/// （含重试后）都取最新 ts，且读/FOR UPDATE 时间戳相同。
/// Mirrors `TestPessimisticRCTxnContextProviderTS` for a for-update
/// statement: RC always fetches the newest ts on every statement and after
/// every retry, and both read and for-update ts land on the same value.
#[test]
fn rc_for_update_statements_always_fetch_the_newest_ts() {
    let (runtime, _) = MockRuntime::new(rc_session(1), &[100, 200, 300]);
    let mut provider = init_rc_provider(runtime);

    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    let for_update_ts = provider.GetStmtForUpdateTS().unwrap();
    assert_eq!(read_ts, for_update_ts);
    assert_eq!(read_ts, 200);
    assert_eq!(
        provider.base.base.runtime.session().txn.for_update_ts,
        read_ts
    );

    // The second read should use the newest ts.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, 300);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);

    // A retry should also refresh the ts.
    let (runtime, _) = MockRuntime::new(rc_session(1), &[100, 200, 300]);
    let mut provider = init_rc_provider(runtime);
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    provider.GetStmtReadTS().unwrap();
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady);
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, 300);
}

/// 对应 `TestRCProviderInitialize`：`Default` 激活但不标显式事务，
/// `WithBeginStmt` 标显式并传播因果一致；`BeforeStmt` 仅在首次读 ts 时惰性激活。
/// Mirrors `TestRCProviderInitialize`: `Default` activates without marking
/// the txn explicit, `WithBeginStmt` does, causal consistency propagates,
/// and `BeforeStmt` only activates lazily on the first ts read.
#[test]
fn rc_provider_initialize_tracks_enter_type_and_causal_consistency() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[5, 10]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let provider = init_rc_provider(runtime);
    TxnAssert::active(Some(IsolationLevel::ReadCommitted), true, baseline)
        .check(&provider.base.base);

    let (runtime, _) = MockRuntime::new(SessionState::default(), &[15, 20]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRCTxnContextProvider(Box::new(runtime), true);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    let mut want = TxnAssert::active(Some(IsolationLevel::ReadCommitted), true, baseline);
    want.causal_consistency_only = true;
    want.check(&provider.base.base);

    // `EnterNewTxnDefault` activates a txn but leaves it non-explicit.
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[25, 30]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRCTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    TxnAssert::active(Some(IsolationLevel::ReadCommitted), false, baseline)
        .check(&provider.base.base);

    // `BeforeStmt` with autocommit off only activates -- and marks the txn
    // explicit -- once the first statement actually reads a ts.
    let session = SessionState {
        autocommit: false,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[35, 40]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRCTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    TxnAssert::inactive(Some(IsolationLevel::ReadCommitted)).check(&provider.base.base);
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let ts = provider.GetStmtReadTS().unwrap();
    TxnAssert::active(Some(IsolationLevel::ReadCommitted), true, baseline)
        .check(&provider.base.base);
    assert_eq!(ts, provider.base.base.runtime.session().txn.start_ts);
}

/// 对应 `TestTidbSnapshotVarInRC`：设置 `tidb_snapshot` 时覆盖 info schema
/// 与时间戳；清除后恢复向 Oracle 取新时间戳。
/// Mirrors `TestTidbSnapshotVarInRC`: `tidb_snapshot` overrides both the
/// info schema and the ts while set, and reads resume fetching fresh
/// oracle timestamps once it is cleared.
#[test]
fn rc_prefers_tidb_snapshot_vars_over_the_live_txn() {
    let snapshot_schema: TxnInfoSchemaRef = std::rc::Rc::new(MockInfoSchema {
        version: 9,
        extended: false,
    });
    let snapshot_ts = 55;
    let session = SessionState {
        snapshot_ts,
        snapshot_info_schema: Some(snapshot_schema),
        ..rc_session(1)
    };
    // `WithBeginStmt` still refreshes the txn's own start ts from the
    // oracle even while `tidb_snapshot` is set (see the comment in
    // `optimistic_test.rs`), so a real ts is queued alongside a follow-up
    // fresh read for after the snapshot is cleared.
    let (runtime, _) = MockRuntime::new(session, &[999, 1000]);
    let mut provider = init_rc_provider(runtime);
    assert!(provider.base.base.runtime.session().txn.start_ts > snapshot_ts);

    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    assert_eq!(
        provider.base.base.GetTxnInfoSchema().schema_meta_version(),
        9
    );
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, snapshot_ts);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);

    // Clearing `tidb_snapshot` restores normal (always-fresh, for a
    // non-read-only statement) txn-scoped reads.
    provider.base.base.runtime.session_mut().snapshot_ts = 0;
    provider
        .base
        .base
        .runtime
        .session_mut()
        .snapshot_info_schema = None;
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let txn_start_ts = provider.base.base.runtime.session().txn.start_ts;
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_ne!(read_ts, snapshot_ts);
    assert!(read_ts > txn_start_ts);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);
}

/// 锁等待超时后，`AfterPessimisticLock` 写冲突分支升级为 `LockWaitTimeout`
/// 而非重试，对应 Go `handleAfterPessimisticLockError`。
/// The `AfterPessimisticLock` write-conflict branch escalates to
/// `LockWaitTimeout` once the statement has already waited past
/// `lock_wait_timeout_ms`, mirroring `readcommitted.go`'s
/// `handleAfterPessimisticLockError`.
#[test]
fn rc_write_conflict_after_lock_wait_timeout_elapsed_errors_instead_of_retrying() {
    let session = SessionState {
        lock_wait_timeout_ms: 100,
        lock_wait_elapsed_ms: 100,
        ..rc_session(1)
    };
    let (runtime, _) = MockRuntime::new(session, &[100]);
    let mut provider = init_rc_provider(runtime);
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(
        action,
        StmtErrorAction::Error(TxnError::new(
            TxnErrorKind::LockWaitTimeout,
            "lock wait timeout"
        ))
    );
}

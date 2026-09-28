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

// Rust port of `repeatable_read_test.go`, exercising the real
// `PessimisticRRTxnContextProvider` state machine through the
// `MockRuntime` harness declared in `main_test.rs`.
//
// `TestConflictErrorIn*InRR`, `TestFailedDMLConsistency*`,
// `TestRRWaitTSTimeInSlowLog` and `TestIssue41194` are executor/DML
// integration scenarios (real `INSERT`/`UPDATE`/`DELETE` execution,
// `assertPessimisticLockErr` failpoint bookkeeping, `analyze table`); like
// the analogous read-committed tests, they exercise the executor package
// rather than the `TxnContextProvider` state machine this crate owns, so
// they stay out of scope for this provider-level harness. The `testfork`
// scope sweep in `TestRepeatableReadProviderInitialize` is likewise
// replaced with direct assertions per `EnterNewTxnType`/causal-consistency
// combination below.

//
// 悲观可重复读（RR）事务上下文 Provider 的单元测试。
//
// 对应 Go `repeatable_read_test.go`：覆盖加锁错误后 for_update_ts 刷新与重试、
// 读 ts 锚定 start ts、初始化语义、`tidb_snapshot` 以及按计划跳过取最新 TSO。
// 执行器/DML 集成场景不在本包范围内。
use crate::main_test::*;
use crate::*;

/// 以 `WithBeginStmt` 初始化 RR Provider。
fn init_rr_provider(runtime: MockRuntime) -> PessimisticRRTxnContextProvider {
    let mut provider = NewPessimisticRRTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    provider
}

/// 对应 `TestPessimisticRRErrorHandle`：写冲突与可重试死锁会刷新
/// `for_update_ts` 并重试；不可重试死锁立即报错；其它错误仍刷新 ts
/// 但不重试；查询后错误对 RR 恒为 NoIdea。
/// Mirrors `TestPessimisticRRErrorHandle`: write conflicts and retryable
/// deadlocks refresh `for_update_ts` and retry; a non-retryable deadlock
/// errors immediately without refreshing anything; any other error still
/// refreshes `for_update_ts` (so a following statement is never stuck
/// behind stale data) but never retries; and errors after the query
/// (rather than after the lock) are always a no-op.
#[test]
fn rr_error_handle_refreshes_for_update_ts_and_retries_only_for_lock_conflicts() {
    // Every step below pops exactly one value from this queue, in order;
    // see the trailing comment on each pop for which slot it consumes.
    let queue: Vec<u64> = (1..=16).map(|i| i * 50).collect();
    let (runtime, _) = MockRuntime::new(SessionState::default(), &queue);
    let clock = runtime.oracle_clock();
    let mut provider = init_rr_provider(runtime); // consumes 50

    // Write conflict: retries and refreshes `for_update_ts` from the
    // oracle; `OnStmtRetry` then carries that refreshed ts forward, so a
    // later read returns it without any further oracle round trip.
    let compare_ts = get_oracle_ts(&clock); // 100
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady); // UpdateForUpdateTS consumes 150
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    let compare_ts2 = get_oracle_ts(&clock); // 200
    let ts = provider.GetStmtForUpdateTS().unwrap();
    assert!(ts > compare_ts);
    assert!(compare_ts2 > ts);
    assert_eq!(ts, 150);

    // Unlike `OnStmtRetry`, `OnStmtStart` resets `for_update_ts`, so the
    // next `GetStmtForUpdateTS` must fetch a brand new ts from the oracle.
    let compare_ts = get_oracle_ts(&clock); // 250
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady); // UpdateForUpdateTS consumes 300
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let compare_ts2 = get_oracle_ts(&clock); // 350
    let ts = provider.GetStmtForUpdateTS().unwrap(); // fresh fetch consumes 400
    assert!(ts > compare_ts);
    assert!(ts > compare_ts2);
    assert_eq!(ts, 400);

    // A non-retryable deadlock errors immediately.
    let deadlock = TxnError::new(TxnErrorKind::Deadlock { retryable: false }, "deadlock");
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        deadlock.clone(),
    );
    assert_eq!(action, StmtErrorAction::Error(deadlock));

    // A retryable deadlock behaves like a write conflict.
    let compare_ts = get_oracle_ts(&clock); // 450
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::Deadlock { retryable: true }, "deadlock"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady); // consumes 500
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    let compare_ts2 = get_oracle_ts(&clock); // 550
    let ts = provider.GetStmtForUpdateTS().unwrap();
    assert!(ts > compare_ts);
    assert!(compare_ts2 > ts);
    assert_eq!(ts, 500);

    let compare_ts = get_oracle_ts(&clock); // 600
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::Deadlock { retryable: true }, "deadlock"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady); // consumes 650
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let compare_ts2 = get_oracle_ts(&clock); // 700
    let ts = provider.GetStmtForUpdateTS().unwrap(); // fresh fetch consumes 750
    assert!(ts > compare_ts);
    assert!(ts > compare_ts2);
    assert_eq!(ts, 750);

    // Any other error still refreshes `for_update_ts` (to avoid leaving a
    // stale lock timestamp behind) but is never retried.
    let other = TxnError::new(TxnErrorKind::InvalidTransaction, "other error");
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        other.clone(),
    ); // UpdateForUpdateTS consumes 800 -- the last queued value
    assert_eq!(action, StmtErrorAction::Error(other));

    // Errors after the query (rather than after the lock) are always a
    // no-op for RR and never touch the ts.
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterQuery,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::NoIdea);
}

/// 锁等待超时后写冲突升级为 `LockWaitTimeout`，不再刷新 for_update_ts 或重试。
/// The write-conflict branch escalates to `LockWaitTimeout` once the
/// statement has already waited past `lock_wait_timeout_ms`, instead of
/// refreshing `for_update_ts` and retrying.
#[test]
fn rr_write_conflict_after_lock_wait_timeout_elapsed_errors_instead_of_retrying() {
    let session = SessionState {
        lock_wait_timeout_ms: 100,
        lock_wait_elapsed_ms: 100,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[10]);
    let mut provider = init_rr_provider(runtime);
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

/// 对应 `TestRepeatableReadProviderTS`：读 ts 在整个事务内锚定 start ts
/// （含 `OnStmtRetry`）；FOR UPDATE ts 按需从 Oracle 取新值。
/// Mirrors `TestRepeatableReadProviderTS`: the read ts is pinned to the txn
/// start ts for the whole transaction (even across `OnStmtRetry`), while
/// the for-update ts is fetched fresh from the oracle on demand.
#[test]
fn rr_read_ts_is_pinned_to_start_ts_while_for_update_ts_is_fetched_on_demand() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[100, 200]);
    let mut provider = init_rr_provider(runtime); // consumes 100 (start ts)

    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, 100);

    // A new statement observes the same read ts.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), read_ts);

    // `OnStmtRetry` does not change the read ts either.
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), read_ts);

    // A for-update statement fetches a fresh, newer ts...
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let for_update_ts = provider.GetStmtForUpdateTS().unwrap();
    assert!(for_update_ts > read_ts);
    assert_eq!(for_update_ts, 200);

    // ...but the read ts is still pinned to the original start ts.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), read_ts);
}

/// 对应 `TestRepeatableReadProviderInitialize`：按进入类型与因果一致标志
/// 校验激活/显式事务行为；`BeforeStmt`+autocommit=0 惰性激活。
/// Mirrors `TestRepeatableReadProviderInitialize`: `Default` activates
/// without marking the txn explicit; `WithBeginStmt` does, and propagates
/// causal consistency; `BeforeStmt` with autocommit off only activates (and
/// becomes explicit) lazily, on the first ts read.
#[test]
fn rr_provider_initialize_tracks_enter_type_and_causal_consistency() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[5, 10]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let provider = init_rr_provider(runtime);
    TxnAssert::active(Some(IsolationLevel::RepeatableRead), true, baseline)
        .check(&provider.base.base);

    let (runtime, _) = MockRuntime::new(SessionState::default(), &[15, 20]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRRTxnContextProvider(Box::new(runtime), true);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    let mut want = TxnAssert::active(Some(IsolationLevel::RepeatableRead), true, baseline);
    want.causal_consistency_only = true;
    want.check(&provider.base.base);

    let (runtime, _) = MockRuntime::new(SessionState::default(), &[25, 30]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRRTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    TxnAssert::active(Some(IsolationLevel::RepeatableRead), false, baseline)
        .check(&provider.base.base);

    let session = SessionState {
        autocommit: false,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[35, 40]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRRTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    TxnAssert::inactive(Some(IsolationLevel::RepeatableRead)).check(&provider.base.base);
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let ts = provider.GetStmtReadTS().unwrap();
    TxnAssert::active(Some(IsolationLevel::RepeatableRead), true, baseline)
        .check(&provider.base.base);
    assert_eq!(ts, provider.base.base.runtime.session().txn.start_ts);
}

/// 对应 `TestTidbSnapshotVarInPessimisticRepeatableRead`：`tidb_snapshot`
/// 覆盖 info schema 与读/FOR UPDATE ts；清除后恢复锚定读 ts 与新取的 for_update_ts。
/// Mirrors `TestTidbSnapshotVarInPessimisticRepeatableRead`: `tidb_snapshot`
/// overrides both info schema and read/for-update ts while set; clearing it
/// restores the pinned read ts and a freshly fetched for-update ts.
#[test]
fn rr_prefers_tidb_snapshot_vars_over_the_live_txn() {
    let snapshot_schema: TxnInfoSchemaRef = std::rc::Rc::new(MockInfoSchema {
        version: 3,
        extended: false,
    });
    let snapshot_ts = 40;
    let session = SessionState {
        snapshot_ts,
        snapshot_info_schema: Some(snapshot_schema),
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[999, 1500]);
    let mut provider = init_rr_provider(runtime); // WithBeginStmt still refreshes from the oracle: 999
    assert!(provider.base.base.runtime.session().txn.start_ts > snapshot_ts);

    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    assert_eq!(
        provider.base.base.GetTxnInfoSchema().schema_meta_version(),
        3
    );
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, snapshot_ts);
    let for_update_ts = provider.GetStmtForUpdateTS().unwrap();
    assert_eq!(for_update_ts, read_ts);

    // Clearing `tidb_snapshot` restores the pinned start ts for reads and a
    // freshly fetched (larger) ts for for-update reads.
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
    assert_eq!(read_ts, txn_start_ts);
    let for_update_ts = provider.GetStmtForUpdateTS().unwrap(); // fresh fetch: 1500
    assert!(for_update_ts > read_ts);
    assert_eq!(for_update_ts, 1500);
}

/// 对应 `TestOptimizeWithPlanInPessimisticRR` 的计划形状部分：加锁 PointGet
/// 可复用上次 for_update_ts；全表加锁扫描必须重新向 Oracle 取数。
/// Mirrors the plan-shape half of `TestOptimizeWithPlanInPessimisticRR`:
/// a locking point-get plan (as produced by `delete`/`update`/`... for
/// update` on a single row) lets `for_update_ts` reuse the last value
/// fetched for the transaction, while a full-table locking scan always
/// forces a fresh oracle round trip.
#[test]
fn rr_optimize_with_plan_reuses_for_update_ts_only_for_locking_point_gets() {
    let point_get_for_update = TestPlan::leaf(PlanKind::PointGet {
        lock: true,
        no_second_read: false,
        cache_table: false,
    });
    let delete_point_get = TestPlan::with_child(PlanKind::Delete, point_get_for_update);
    let full_table_scan_for_update = TestPlan::with_child(
        PlanKind::Physical { lock: true },
        TestPlan::leaf(PlanKind::PhysicalTableReader {
            primary_key_point_get: false,
        }),
    );
    assert!(NotNeedGetLatestTSFromPD(&delete_point_get, false));
    assert!(!NotNeedGetLatestTSFromPD(
        &full_table_scan_for_update,
        false
    ));

    let (runtime, _) = MockRuntime::new(SessionState::default(), &[100, 200, 300]);
    let mut provider = init_rr_provider(runtime); // consumes 100

    // First for-update fetch establishes `lastFetchedForUpdateTS`.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let last_fetched = provider.GetStmtForUpdateTS().unwrap(); // consumes 200
    assert_eq!(last_fetched, 200);

    // A locking point-get plan (e.g. `delete ... where id = 1`) reuses that
    // value instead of asking the oracle again.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    provider.AdviseOptimizeWithPlan(&delete_point_get);
    let ts = provider.GetStmtForUpdateTS().unwrap();
    assert_eq!(ts, last_fetched);

    // A full unfiltered `... for update` table scan cannot reuse the plan
    // shortcut and must fetch a genuinely fresh ts.
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    provider.AdviseOptimizeWithPlan(&full_table_scan_for_update);
    let ts = provider.GetStmtForUpdateTS().unwrap(); // consumes 300
    assert!(ts > last_fetched);
    assert_eq!(ts, 300);
}

/// 对应 `TestOptimizeWithPlanInPessimisticRR` 的 autocommit=0 尾部：
/// 无显式 BEGIN 时，激活后首次 FOR UPDATE 读直接返回已取到的 start ts。
/// Mirrors the `autocommit=0` tail of `TestOptimizeWithPlanInPessimisticRR`:
/// with autocommit off and no explicit begin, the very first for-update
/// read after activation just returns the (already fetched) txn start ts,
/// while a read-only statement without a locking plan still fetches a
/// fresh ts as usual.
#[test]
fn rr_optimize_with_plan_is_skipped_before_the_first_activation_when_autocommit_is_off() {
    let session = SessionState {
        autocommit: false,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[450, 500]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticRRTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    let update_point_get = TestPlan::with_child(
        PlanKind::Update,
        TestPlan::leaf(PlanKind::PointGet {
            lock: true,
            no_second_read: false,
            cache_table: false,
        }),
    );
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    provider.AdviseOptimizeWithPlan(&update_point_get);
    let ts = provider.GetStmtForUpdateTS().unwrap(); // first-ever activation: consumes 500
    assert_eq!(ts, provider.base.base.runtime.session().txn.start_ts);
    TxnAssert::active(Some(IsolationLevel::RepeatableRead), true, baseline)
        .check(&provider.base.base);
}

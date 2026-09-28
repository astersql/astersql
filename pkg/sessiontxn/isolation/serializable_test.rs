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

// Rust port of `serializable_test.go`, exercising the real
// `PessimisticSerializableTxnContextProvider` state machine through the
// `MockRuntime` harness declared in `main_test.rs`.
//
// `TestTidbSnapshotVarInSerialize`'s `INSERT`-statement sub-scenarios and
// `TestSerializableInitialize`'s `testfork` scope sweep are executor/DML
// and session-scope integration scenarios (real `INSERT`, TiKV client
// scope injection) the same way their `readcommitted_test.rs`/
// `repeatable_read_test.rs` counterparts are; the plain begin/initialize
// and `tidb_snapshot` provider-level assertions those tests also make are
// fully covered below against the real provider.

//
// 悲观可串行化（Serializable）事务上下文 Provider 的单元测试。
//
// 对应 Go `serializable_test.go`：覆盖读/FOR UPDATE ts 均锚定 start ts、
// 加锁错误永不重试、初始化语义与 `tidb_snapshot` 覆盖。
use crate::main_test::*;
use crate::*;

/// 以 `WithBeginStmt` 初始化可串行化 Provider。
fn init_serializable_provider(runtime: MockRuntime) -> PessimisticSerializableTxnContextProvider {
    let mut provider = NewPessimisticSerializableTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    provider
}

/// 对应 `TestPessimisticSerializableTxnProviderTS`：可串行化下读 ts 与
/// FOR UPDATE ts 相同，整个事务内均锚定 start ts。
/// Mirrors `TestPessimisticSerializableTxnProviderTS`: in serializable
/// isolation, the read ts and the for-update ts are the same value, both
/// pinned to the txn's start ts for the whole transaction.
#[test]
fn serializable_read_ts_and_for_update_ts_are_both_pinned_to_the_start_ts() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[100, 200]);
    let clock = runtime.oracle_clock();
    let mut provider = init_serializable_provider(runtime); // consumes 100
    let compare_ts = get_oracle_ts(&clock); // fresh pop, strictly newer: 200

    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let ts = provider.GetStmtReadTS().unwrap();
    assert!(compare_ts > ts);
    let prev_ts = ts;

    // In Oracle-like serializable isolation, readTS equals the for-update ts.
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
        .unwrap();
    let ts = provider.GetStmtForUpdateTS().unwrap();
    assert!(compare_ts > ts);
    assert_eq!(prev_ts, ts);
}

/// 对应 `TestPessimisticSerializableTxnContextProviderLockError`：
/// 可串行化事务永不重试，加锁后错误一律原样返回，其它切入点为 NoIdea。
/// Mirrors `TestPessimisticSerializableTxnContextProviderLockError`:
/// serializable transactions never retry, so every
/// `StmtErrAfterPessimisticLock` error (retryable or not) surfaces
/// unchanged, and every other handle point is a no-op.
#[test]
fn serializable_never_retries_and_always_surfaces_the_lock_error_unchanged() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[10]);
    let mut provider = init_serializable_provider(runtime);

    for error in [
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
        TxnError::new(TxnErrorKind::Deadlock { retryable: true }, "deadlock"),
        TxnError::new(TxnErrorKind::Deadlock { retryable: false }, "deadlock"),
        TxnError::new(TxnErrorKind::InvalidTransaction, "err"),
    ] {
        provider
            .base
            .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
            .unwrap();
        let action = provider
            .OnStmtErrorForNextAction(StmtErrorHandlePoint::AfterPessimisticLock, error.clone());
        assert_eq!(action, StmtErrorAction::Error(error));
    }

    let action = provider.OnStmtErrorForNextAction(
        StmtErrorHandlePoint::AfterQuery,
        TxnError::new(TxnErrorKind::WriteConflict, "write conflict"),
    );
    assert_eq!(action, StmtErrorAction::NoIdea);
}

/// 对应 `TestSerializableInitialize`：按进入类型与因果一致标志校验激活行为。
/// Mirrors `TestSerializableInitialize`: `WithBeginStmt` activates and
/// marks the txn explicit; causal consistency propagates; `Default`
/// activates without marking the txn explicit; and `BeforeStmt` with
/// autocommit off only activates -- and becomes explicit -- lazily on the
/// first ts read.
#[test]
fn serializable_initialize_tracks_enter_type_and_causal_consistency() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[10, 20]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticSerializableTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    TxnAssert::active(Some(IsolationLevel::Serializable), true, baseline).check(&provider.base);

    let (runtime, _) = MockRuntime::new(SessionState::default(), &[25, 30]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticSerializableTxnContextProvider(Box::new(runtime), true);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    let mut want = TxnAssert::active(Some(IsolationLevel::Serializable), true, baseline);
    want.causal_consistency_only = true;
    want.check(&provider.base);

    // `EnterNewTxnDefault` activates a txn but leaves it non-explicit.
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[35, 40]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticSerializableTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    TxnAssert::active(Some(IsolationLevel::Serializable), false, baseline).check(&provider.base);

    // `BeforeStmt` with autocommit off only activates -- and marks the txn
    // explicit -- once the first statement actually reads a ts.
    let session = SessionState {
        autocommit: false,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[45, 50]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticSerializableTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    TxnAssert::inactive(Some(IsolationLevel::Serializable)).check(&provider.base);
    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    let ts = provider.GetStmtReadTS().unwrap();
    TxnAssert::active(Some(IsolationLevel::Serializable), true, baseline).check(&provider.base);
    assert_eq!(ts, provider.base.runtime.session().txn.start_ts);
}

/// 对应 `TestTidbSnapshotVarInSerialize` 的 `tidb_snapshot` 部分：
/// 设置时按快照读；清除后恢复锚定的事务 start ts。
/// Mirrors the `tidb_snapshot` half of `TestTidbSnapshotVarInSerialize`:
/// while `tidb_snapshot` is set, both info schema and ts read through the
/// snapshot; clearing it restores the pinned txn start ts.
#[test]
fn serializable_prefers_tidb_snapshot_vars_over_the_live_txn() {
    let snapshot_schema: TxnInfoSchemaRef = std::rc::Rc::new(MockInfoSchema {
        version: 6,
        extended: false,
    });
    let snapshot_ts = 40;
    let session = SessionState {
        snapshot_ts,
        snapshot_info_schema: Some(snapshot_schema),
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[999]);
    let mut provider = init_serializable_provider(runtime);
    assert!(provider.base.runtime.session().txn.start_ts > snapshot_ts);

    provider
        .base
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.base.GetTxnInfoSchema().schema_meta_version(), 6);
    let read_ts = provider.GetStmtReadTS().unwrap();
    assert_eq!(read_ts, snapshot_ts);
    assert_eq!(provider.GetStmtForUpdateTS().unwrap(), read_ts);

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

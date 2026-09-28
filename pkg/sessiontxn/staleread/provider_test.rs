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

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// `StalenessTxnContextProvider` coverage for `pkg/sessiontxn/staleread`.
//
// The Go suite (`provider_test.go`) drives
// `NewStalenessTxnContextProvider`/`OnInitialize`/`GetSnapshotWithStmtReadTS`
// through a real `testkit` session (transaction scope, follower-read
// snapshot interceptors, autocommit activation, ...). This crate's Rust
// port (`provider.rs`) keeps the exact same activation/snapshot/txn-scope
// logic behind the `SessionBackend` trait, so this file drives the real
// `StalenessTxnContextProvider` state machine against the `MockBackend`
// harness from `main_test.rs` instead of a real session/store.

//
// 中文概述：覆盖 `StalenessTxnContextProvider`。
// 验证初始化激活、替换 Provider、语句读 ts 自动激活、
// 快照复用/新建，以及 Follower 读偏好透传。

use crate::main_test::*;
use crate::*;

#[test]
/// Default 进入类型激活只读过期读事务与上下文。
fn on_initialize_default_activates_a_read_only_stale_transaction() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session.clone(), 1234, None);

    provider
        .on_initialize(Context, EnterNewTxnType::Default)
        .expect("activating a stale transaction must succeed");

    assert_eq!(provider.txn_info_schema().unwrap().snapshot_ts, 1234);
    assert_eq!(provider.txn_scope(), GLOBAL_TXN_SCOPE);
    assert_eq!(
        backend
            .calls
            .lock()
            .unwrap()
            .commit_before_enter_new_txn_calls,
        1
    );
    assert_eq!(
        backend.calls.lock().unwrap().create_transaction_ts,
        vec![1234]
    );

    let state = session.lock().unwrap();
    let txn_context = state
        .txn_context
        .as_ref()
        .expect("a transaction context must have been installed");
    assert!(txn_context.is_staleness);
    assert_eq!(txn_context.start_ts, 1234);
    assert_eq!(txn_context.txn_scope, GLOBAL_TXN_SCOPE);
    let active = state
        .active_transaction
        .as_ref()
        .expect("an active transaction must have been installed");
    assert!(active.staleness_read_only);
    assert_eq!(active.txn_scope, GLOBAL_TXN_SCOPE);
    assert_eq!(active.shard_allocate_step, 0);
    assert!(active.snapshot.staleness_read_only);
}

#[test]
/// WithBeginStatement 与 Default 同样激活事务。
fn on_initialize_with_begin_statement_also_activates_a_stale_transaction() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session.clone(), 42, None);

    provider
        .on_initialize(Context, EnterNewTxnType::WithBeginStatement)
        .expect("`WithBeginStatement` must activate a stale transaction the same as `Default`");

    assert_eq!(
        backend.calls.lock().unwrap().create_transaction_ts,
        vec![42]
    );
    assert!(session.lock().unwrap().active_transaction.is_some());
}

#[test]
/// WithReplaceProvider 只装 txn_context，不 create_transaction。
fn on_initialize_with_replace_provider_installs_txn_context_without_activating() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session.clone(), 77, None);

    provider
        .on_initialize(Context, EnterNewTxnType::WithReplaceProvider)
        .expect("replacing the provider must succeed without activating a transaction");

    assert_eq!(
        backend.calls.lock().unwrap().create_transaction_ts,
        Vec::<u64>::new()
    );
    let state = session.lock().unwrap();
    assert!(state.active_transaction.is_none());
    let txn_context = state
        .txn_context
        .as_ref()
        .expect("a transaction context must still be installed");
    assert!(txn_context.is_staleness);
    assert_eq!(txn_context.info_schema.snapshot_ts, 77);
}

#[test]
/// Unsupported 进入类型返回 Unsupported 错误。
fn on_initialize_unsupported_returns_an_unsupported_error() {
    let (session, _backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session, 1, None);

    let error = provider
        .on_initialize(Context, EnterNewTxnType::Unsupported)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
}

#[test]
/// ForUpdateTS 永不支持。
fn stmt_for_update_ts_is_never_supported() {
    let (session, _backend) = mock_session();
    let provider = StalenessTxnContextProvider::new(session, 1, None);
    let error = provider.stmt_for_update_ts().unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
}

#[test]
/// ForUpdate 快照永不支持。
fn snapshot_with_stmt_for_update_ts_is_never_supported() {
    let (session, _backend) = mock_session();
    let provider = StalenessTxnContextProvider::new(session, 1, None);
    let error = provider.snapshot_with_stmt_for_update_ts().unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
}

#[test]
/// 语句错误后恒为 NoIdea。
fn on_stmt_error_for_next_action_always_reports_no_idea() {
    let (session, _backend) = mock_session();
    let provider = StalenessTxnContextProvider::new(session, 1, None);
    let (action, error) = provider.on_stmt_error_for_next_action(&Error::as_of("boom"));
    assert_eq!(action, StatementErrorAction::NoIdea);
    assert!(error.is_none());
}

#[test]
/// 读副本作用域来自配置，默认值为 global。
fn read_replica_scope_comes_from_the_configured_txn_scope() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().txn_scope_config = "bj".to_owned();
    let provider = StalenessTxnContextProvider::new(session, 1, None);
    assert_eq!(provider.read_replica_scope(), "bj");
}

#[test]
/// txn_scope 来自会话事务上下文。
fn txn_scope_reads_from_the_session_transaction_context_when_present() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().txn_context = Some(TransactionContext {
        info_schema: InfoSchema::default(),
        start_ts: 1,
        is_staleness: true,
        txn_scope: "dc-1".to_owned(),
    });
    let provider = StalenessTxnContextProvider::new(session, 1, None);
    assert_eq!(provider.txn_scope(), "dc-1");
}

#[test]
/// 无事务上下文时 txn_scope 为空。
fn txn_scope_is_empty_without_a_transaction_context() {
    let (session, _backend) = mock_session();
    let provider = StalenessTxnContextProvider::new(session, 1, None);
    assert_eq!(provider.txn_scope(), "");
}

#[test]
/// autocommit=0 且未进事务时 stmt_read_ts 自动激活。
fn stmt_read_ts_auto_activates_when_autocommit_is_off_and_not_in_a_transaction() {
    let (session, backend) = mock_session();
    session.lock().unwrap().autocommit = false;
    let mut provider = StalenessTxnContextProvider::new(session.clone(), 55, None);

    let ts = provider
        .stmt_read_ts()
        .expect("autocommit=0 outside a transaction must auto-activate");

    assert_eq!(ts, 55);
    assert_eq!(
        backend.calls.lock().unwrap().create_transaction_ts,
        vec![55]
    );
    assert!(session.lock().unwrap().in_txn);
}

#[test]
/// 已在事务中时不再重新激活。
fn stmt_read_ts_does_not_reactivate_when_already_in_a_transaction() {
    let (session, backend) = mock_session();
    {
        let mut state = session.lock().unwrap();
        state.autocommit = false;
        state.in_txn = true;
    }
    let mut provider = StalenessTxnContextProvider::new(session, 55, None);

    let ts = provider.stmt_read_ts().unwrap();

    assert_eq!(ts, 55);
    assert!(
        backend
            .calls
            .lock()
            .unwrap()
            .create_transaction_ts
            .is_empty(),
        "an already-active transaction must not be recreated"
    );
}

#[test]
/// autocommit 开启时不自动激活事务。
fn stmt_read_ts_does_not_activate_under_autocommit() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session, 55, None);

    let ts = provider.stmt_read_ts().unwrap();

    assert_eq!(ts, 55);
    assert!(
        backend
            .calls
            .lock()
            .unwrap()
            .create_transaction_ts
            .is_empty()
    );
}

#[test]
/// activate_txn 只创建一次并缓存。
fn activate_txn_creates_the_transaction_only_once() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session, 88, None);

    let first = provider.activate_txn().unwrap();
    let second = provider.activate_txn().unwrap();

    assert_eq!(first, second);
    assert_eq!(
        backend.calls.lock().unwrap().create_transaction_ts,
        vec![88]
    );
}

#[test]
/// 有活跃事务时复用其快照，不再问后端。
fn snapshot_with_stmt_read_ts_reuses_the_active_transaction_snapshot() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session, 99, None);
    provider.activate_txn().unwrap();

    let snapshot = provider.snapshot_with_stmt_read_ts().unwrap();

    assert!(snapshot.staleness_read_only);
    assert!(
        backend
            .calls
            .lock()
            .unwrap()
            .snapshot_with_ts_calls
            .is_empty(),
        "an active transaction's own snapshot must be reused instead of asking the backend again"
    );
}

#[test]
/// 无活跃事务时向后端按 ts 取快照。
fn snapshot_with_stmt_read_ts_asks_the_backend_without_an_active_transaction() {
    let (session, backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session, 99, None);

    let snapshot = provider.snapshot_with_stmt_read_ts().unwrap();

    assert!(snapshot.staleness_read_only);
    assert_eq!(
        backend.calls.lock().unwrap().snapshot_with_ts_calls,
        vec![99]
    );
}

#[test]
/// 配置 Follower 读时透传到快照。
fn snapshot_with_stmt_read_ts_marks_follower_read_when_configured() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().replica_read = ReplicaRead::Follower;
    let mut provider = StalenessTxnContextProvider::new(session, 99, None);

    let snapshot = provider.snapshot_with_stmt_read_ts().unwrap();

    assert_eq!(snapshot.replica_read, ReplicaRead::Follower);
}

#[test]
/// Provider 只有安装/激活后才标记为当前过期读 Provider。
fn provider_is_marked_when_it_is_installed() {
    let (session, _backend) = mock_session();
    let mut provider = StalenessTxnContextProvider::new(session.clone(), 1, None);
    assert!(!session.lock().unwrap().provider_is_staleness);

    provider
        .on_initialize(Context, EnterNewTxnType::WithReplaceProvider)
        .unwrap();
    assert!(session.lock().unwrap().provider_is_staleness);
}

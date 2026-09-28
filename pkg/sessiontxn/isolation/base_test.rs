// Copyright 2026 AsterSQL.

use crate::main_test::{MockRuntime, TestStatement};
use crate::*;

#[test]
fn base_statement_hooks_match_go_side_effects() {
    let mut session = SessionState::default();
    session.txn.flushed_stmt_lock_cache = true;
    let (runtime, _) = MockRuntime::new(session, &[]);
    let mut provider =
        BaseTxnContextProvider::new(Box::new(runtime), IsolationLevel::Optimistic, false, false);

    provider
        .OnStmtStart(
            RuntimeContext {
                request_id: "start".into(),
                cancelled: false,
            },
            &TestStatement(true),
        )
        .unwrap();
    assert_eq!(provider.context.request_id, "start");
    assert!(provider.runtime.session().txn.flushed_stmt_lock_cache);

    provider
        .OnStmtCommit(RuntimeContext {
            request_id: "commit".into(),
            cancelled: false,
        })
        .unwrap();
    assert_eq!(provider.context.request_id, "start");

    provider
        .OnStmtRollback(
            RuntimeContext {
                request_id: "rollback".into(),
                cancelled: false,
            },
            false,
        )
        .unwrap();
    assert_eq!(provider.context.request_id, "start");
}

// Copyright 2026 AsterSQL.

//! Go/Rust parity tests for the public transaction-manager interface.

use crate::txn_manager_test::{MockSession, request_context};
use crate::{Error, NewTxnInStmt, RequestContext, TxnManager};

fn rollback_result(manager: &mut dyn TxnManager, context: &RequestContext) -> Result<(), Error> {
    manager.OnStmtRollback(context, false)
}

#[test]
fn rollback_hook_preserves_the_go_error_contract() {
    let (mut session, _calls) = MockSession::new();
    rollback_result(&mut session.manager, &request_context())
        .expect("the mock rollback hook should succeed");

    session.manager.provider.fail_stmt_rollback = true;
    let error = rollback_result(&mut session.manager, &request_context())
        .expect_err("the provider rollback error must reach the caller");
    assert_eq!(error.to_string(), "stmt rollback failed");
}

#[test]
fn new_txn_in_stmt_calls_stmt_start_even_without_a_current_statement() {
    let (mut session, calls) = MockSession::new();

    NewTxnInStmt(&request_context(), &mut session)
        .expect("NewTxnInStmt must succeed against the mock manager");

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:enter-new-txn:EnterNewTxnDefault".to_owned(),
            "provider:on-initialize:EnterNewTxnDefault".to_owned(),
            "manager:on-stmt-start".to_owned(),
            "provider:on-stmt-start".to_owned(),
        ],
    );
}

// Copyright 2026 AsterSQL.

use std::sync::Arc;

/// Go 的 InternalSctxForTest 只读取内部上下文，不要求调用方仍是 owner。
#[test]
fn internal_sctx_for_test_remains_available_after_close() {
    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create session");

    session.Close();

    assert!(session.InternalSctxForTest().is_ok());
}

/// Go 的 ResetSctxForTest 仍须验证 owner，关闭后不得替换上下文。
#[test]
fn reset_sctx_for_test_rejects_closed_session() {
    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create session");
    session.Close();

    let error = session
        .ResetSctxForTest(|_| panic!("closed session must not invoke replacement"))
        .expect_err("closed session must fail owner validation");
    assert_eq!(error.to_string(), "session is not owned by the caller");
}

// Copyright 2026 AsterSQL.

use crate::harness::{CreateMockStoreAndSetup, TestCtx, reset_engine, serial_guard, testkit};

#[test]
fn must_exec_rejects_unsupported_sql() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = testkit::NewTestKit(&t, CreateMockStoreAndSetup(&t));

    let panic = std::panic::catch_unwind(|| tk.MustExec("creat table typo(a int)"));
    assert!(
        panic.is_err(),
        "MustExec must not silently accept invalid SQL"
    );
}

#[test]
fn must_query_rejects_unsupported_sql() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = testkit::NewTestKit(&t, CreateMockStoreAndSetup(&t));

    let panic = std::panic::catch_unwind(|| tk.MustQuery("show unsupported_statistic"));
    assert!(
        panic.is_err(),
        "MustQuery must not turn invalid SQL into empty rows"
    );
}

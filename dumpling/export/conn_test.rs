// Copyright 2026 AsterSQL.

use crate::*;

#[test]
fn exec_sql_uses_callback_result_for_execution_errors() {
    let conn = Conn::new();
    conn.push_fail(errors_new("unknown column 1054"));
    let mut base = newBaseConn(conn, false, None);
    let tctx = tcontext::Background();

    let result = base.ExecSQL(
        &tctx,
        |_result, err| {
            assert_eq!(
                err.map(|error| error.msg.as_str()),
                Some("unknown column 1054")
            );
            Ok(())
        },
        "SELECT missing_column",
    );

    assert!(result.is_ok());
}

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

#[test]
fn query_rows_handles_empty_results_and_retries_with_reset_and_rebuild() {
    use std::cell::RefCell;
    let conn = Conn::new();
    conn.seed_rows("metadata", Rows::new(vec!["id".into()], vec![]));
    let mut base = newBaseConn(conn, false, None);
    let calls = RefCell::new(0);
    base.queryRows(
        &tcontext::Background(),
        |rows| {
            *calls.borrow_mut() += 1;
            assert_eq!(rows.ColumnTypes()?.len(), 1);
            Ok(())
        },
        || panic!("unexpected reset"),
        "metadata",
    )
    .unwrap();
    assert_eq!(*calls.borrow(), 1);

    let failed = Conn::new();
    let mut rows = Rows::new(vec!["id".into()], vec![vec![Some(b"1".to_vec())]]);
    rows.err = Some(errors_new("row failure"));
    failed.seed_rows("rows", rows);
    let rebuilt = Conn::new();
    rebuilt.seed_rows(
        "rows",
        Rows::new(vec!["id".into()], vec![vec![Some(b"2".to_vec())]]),
    );
    let rebuild_count = Arc::new(AtomicU64::new(0));
    let count = rebuild_count.clone();
    let mut base = newBaseConn(
        failed,
        true,
        Some(Box::new(move |_, update| {
            assert!(!update);
            count.fetch_add(1, Ordering::SeqCst);
            Ok(rebuilt.clone())
        })),
    );
    let values = RefCell::new(Vec::new());
    let resets = RefCell::new(0);
    base.QuerySQL(
        &tcontext::Background(),
        |rows| {
            let mut dest = [RawBytes(None)];
            rows.Scan(&mut dest)?;
            values.borrow_mut().push(dest[0].as_opt().unwrap().to_vec());
            Ok(())
        },
        || {
            values.borrow_mut().clear();
            *resets.borrow_mut() += 1;
        },
        "rows",
    )
    .unwrap();
    assert_eq!(*values.borrow(), vec![b"2".to_vec()]);
    assert_eq!(*resets.borrow(), 1);
    assert_eq!(rebuild_count.load(Ordering::SeqCst), 1);
    assert_eq!(base.backOffer.RemainingAttempts(), dumpChunkRetryTime);
}

#[test]
fn query_columns_retries_close_and_propagates_final_row_errors() {
    let conn = Conn::new();
    let mut first = Rows::new(vec!["id".into()], vec![vec![Some(b"1".to_vec())]]);
    first.close_error = Some(errors_new("close failure"));
    conn.seed_rows("query", first);
    conn.seed_rows(
        "query",
        Rows::new(vec!["id".into()], vec![vec![Some(b"2".to_vec())]]),
    );
    let mut base = newBaseConn(conn, true, None);
    assert_eq!(
        base.QuerySQLWithColumns(&tcontext::Background(), &["id"], "query")
            .unwrap(),
        vec![vec!["2"]]
    );
    let conn = Conn::new();
    let mut failed = Rows::new(vec!["id".into()], vec![]);
    failed.err = Some(errors_new("driver row error"));
    conn.seed_rows("failed", failed);
    let mut base = newBaseConn(conn, false, None);
    let error = base
        .QuerySQLWithColumns(&tcontext::Background(), &["id"], "failed")
        .unwrap_err();
    assert!(error.msg.contains("driver row error"));
    assert!(error.msg.contains("sql: failed, args: []"));
}

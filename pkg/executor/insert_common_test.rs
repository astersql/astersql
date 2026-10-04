// Copyright 2026 AsterSQL.

use super::insert_common::resolve_get_row_cast_error;

#[test]
fn get_row_cast_error_follows_go_error_context_result() {
    assert_eq!(resolve_get_row_cast_error(false, "cast", Ok(())), Ok(()));
    assert_eq!(resolve_get_row_cast_error(true, "cast", Ok(())), Ok(()));
    assert_eq!(
        resolve_get_row_cast_error(false, "cast", Err("completed")),
        Err("cast")
    );
    assert_eq!(
        resolve_get_row_cast_error(true, "cast", Err("completed")),
        Err("completed")
    );
}

#[test]
fn terminal_auto_id_marker_survives_error_wrapping() {
    use super::insert_common::is_terminal_auto_id_error;
    use astersql_meta_autoid::AutoIdError;
    let marked =
        astersql_errors::SharedError::new(AutoIdError::RpcRetryLimit("last RPC failure".into()));
    assert!(is_terminal_auto_id_error(&marked));
    let ordinary = astersql_errors::SharedError::new(AutoIdError::AutoIncrementReadFailed(
        "read failed".into(),
    ));
    assert!(!is_terminal_auto_id_error(&ordinary));
}

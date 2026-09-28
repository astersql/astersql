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

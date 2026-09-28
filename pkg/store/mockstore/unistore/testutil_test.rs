// Copyright 2026 AsterSQL.

use crate::rpc::Request;
use crate::testutil::{check_resource_tag_for_top_sql, get_request_start_key};

#[test]
fn empty_request_is_ignored_like_go_cmd_empty() {
    assert_eq!(get_request_start_key(&Request::Empty), Ok(None));
    assert_eq!(
        check_resource_tag_for_top_sql(&Request::Empty, &[], true),
        Ok(())
    );
}

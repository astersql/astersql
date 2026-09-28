// Copyright 2026 AsterSQL.

use crate::closure_exec::ClosureExecutor;
use crate::cop_handler::KeyRange;

#[test]
fn point_range_preserves_carry_suffix_like_go_key_range() {
    let range = KeyRange {
        start: vec![0x01, 0xff],
        end: vec![0x02, 0x00],
    };

    assert!(ClosureExecutor::is_point_get_range(&range));
}

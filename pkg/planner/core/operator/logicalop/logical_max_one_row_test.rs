// Copyright 2026 AsterSQL.

use crate::LogicalMaxOneRow;

#[test]
#[should_panic]
fn schema_requires_a_child_like_go() {
    let _ = LogicalMaxOneRow::default().Schema();
}

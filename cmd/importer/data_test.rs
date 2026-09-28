// Copyright 2026 AsterSQL.

use crate::data::newDatum;

#[test]
fn negative_step_range_initialization_matches_go_overflow() {
    let datum = newDatum();
    datum.set_step(-1);

    datum.setInitInt64Value(i64::MAX, i64::MAX);

    assert_eq!(datum.nextInt64(), i64::MAX);
}

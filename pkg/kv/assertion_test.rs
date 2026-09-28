// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn assertion_operations_cover_go_uint8_domain() {
    assert_eq!(AssertionOp::AssertExist, AssertionOp(0));
    assert_eq!(AssertionOp::AssertNotExist, AssertionOp(1));
    assert_eq!(AssertionOp::AssertUnknown, AssertionOp(2));
    assert_eq!(AssertionOp::AssertNone, AssertionOp(3));
    for origin in 0..=u8::MAX {
        for op in 0..=u8::MAX {
            // Go assertion.go switch, including its implicit default no-op.
            let expected = match op {
                0 => (origin | 4) & !8,
                1 => (origin | 8) & !4,
                2 => origin | 12,
                _ => origin,
            };
            assert_eq!(
                ApplyAssertionOp(KeyFlags(origin), AssertionOp(op)).0,
                expected,
                "origin={origin}, op={op}",
            );
        }
    }
}

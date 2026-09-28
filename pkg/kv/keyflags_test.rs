// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn key_flags_match_go_bit_semantics() {
    let flags = ApplyFlagsOps(
        KeyFlags::default(),
        &[
            FlagsOp::SetPresumeKeyNotExists,
            FlagsOp::SetNeedLocked,
            FlagsOp::SetNeedConstraintCheckInPrewrite,
            FlagsOp::SetPreviousPresumeKeyNotExists,
        ],
    );

    assert!(flags.HasPresumeKeyNotExists());
    assert!(flags.HasNeedLocked());
    assert!(flags.HasNeedConstraintCheckInPrewrite());
    assert_eq!(flags.0, 0b11_0011);

    for bits in 0_u8..=u8::MAX {
        let flags = KeyFlags(bits);
        assert_eq!(flags.HasAssertExists(), bits & 0b1100 == 0b0100);
        assert_eq!(flags.HasAssertNotExists(), bits & 0b1100 == 0b1000);
        assert_eq!(flags.HasAssertUnknown(), bits & 0b1100 == 0b1100);
        assert_eq!(flags.HasAssertionFlags(), bits & 0b1100 != 0);
    }
}

#[test]
fn unknown_flags_op_is_a_no_op_like_go() {
    let origin = KeyFlags(0b1010_0101);
    assert_eq!(ApplyFlagsOps(origin, &[FlagsOp(u16::MAX)]), origin);
}

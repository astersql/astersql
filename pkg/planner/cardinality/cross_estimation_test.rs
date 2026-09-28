// Copyright 2026 AsterSQL.

use crate::*;

fn range_with_two_collators() -> ranger::Range {
    ranger::Range {
        LowVal: vec![types::NewIntDatum(1)],
        HighVal: vec![types::NewIntDatum(2)],
        Collators: collate::GetBinaryCollatorSlice(2),
        ..Default::default()
    }
}

#[test]
fn test_convert_range_preserves_source_collators() {
    let ranges = [range_with_two_collators()];

    let (ascending, _, is_full) = convertRangeFromExpectedCnt(&ranges, &[1.0], 1.0, false);
    assert!(!is_full);
    assert_eq!(ascending[0].Collators.len(), ranges[0].Collators.len());

    let (descending, _, is_full) = convertRangeFromExpectedCnt(&ranges, &[1.0], 1.0, true);
    assert!(!is_full);
    assert_eq!(descending[0].Collators.len(), ranges[0].Collators.len());
}

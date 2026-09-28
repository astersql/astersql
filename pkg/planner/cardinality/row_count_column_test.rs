// Copyright 2026 AsterSQL.

use crate::*;

#[test]
fn partial_stats_reuses_first_index_range_collator() {
    let range = ranger::Range {
        Collators: vec![collate::GetCollator("utf8mb4_general_ci")],
        ..Default::default()
    };

    let collator = partialStatsRangeCollator(&range);
    assert_eq!(collator.Compare("A", "a"), 0);
}

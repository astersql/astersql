// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn go_merge_49_adaptive_purge_respects_rate_budget_and_batch_limit() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(100);
    let plan = MLogPurgeThrottlePlan::new(100_000, deadline, 2_000.0, 0.5)
        .expect("positive pending rows and deadline");
    assert!(plan.target_rate >= 2_000.0);
    assert_eq!(plan.batch_size(10_000), 8_000);
    assert_eq!(plan.batch_size(500), 500);
    assert!(MLogPurgeThrottlePlan::new(0, deadline, 2_000.0, 0.5).is_none());
    assert!(MLogPurgeThrottlePlan::new(100, deadline, 2_000.0, 0.0).is_none());
}

#[test]
fn go_merge_49_adaptive_purge_splits_linear_and_sharded_row_ids() {
    let linear = MLogPendingRowStats {
        pending_rows: 16_000,
        row_id_bounds: Some((0, 15_999)),
    };
    assert_eq!(
        mlog_purge_row_id_ranges(&linear, 0),
        vec![
            MLogRowIDRange {
                start: 0,
                end: 7_999
            },
            MLogRowIDRange {
                start: 8_000,
                end: 15_999
            },
        ]
    );
    let sharded = MLogPendingRowStats {
        pending_rows: 16_000,
        row_id_bounds: Some((0, i64::MAX)),
    };
    assert_eq!(
        mlog_purge_row_id_ranges(&sharded, 1),
        vec![
            MLogRowIDRange {
                start: 0,
                end: (1_i64 << 62) - 1
            },
            MLogRowIDRange {
                start: 1_i64 << 62,
                end: i64::MAX
            },
        ]
    );
}

// Copyright 2026 AsterSQL.

use crate::index_auto_presplit::{
    AutoPreSplitConfig, AutoPreSplitEligibility, AutoPreSplitPlanState, DistributionStats,
    HistogramPoint, plan_auto_pre_split,
};
use crate::index_cop::Datum;

fn config() -> AutoPreSplitConfig {
    AutoPreSplitConfig {
        min_table_rows: 10,
        min_stats_healthy: 80,
        boundary_ratio_step: 0.2,
    }
}

fn eligible() -> AutoPreSplitEligibility {
    AutoPreSplitEligibility {
        table_id: 42,
        index_id: 7,
        has_leading_column: true,
        row_count: 100,
        stats_healthy: Some(100),
        ..Default::default()
    }
}

#[test]
fn topn_and_histogram_are_merged_and_sampled_like_go() {
    let stats = DistributionStats {
        stats_version: 2,
        null_count: 25,
        top_n: vec![
            (Datum::Int(10), 25),
            (Datum::Int(50), 14),
            (Datum::Int(90), 11),
        ],
        histogram: vec![
            HistogramPoint::new(Datum::Int(20), 10),
            HistogramPoint::new(Datum::Int(40), 20),
            HistogramPoint::new(Datum::Int(60), 30),
            HistogramPoint::new(Datum::Int(80), 40),
            HistogramPoint::new(Datum::Int(100), 50),
        ],
    };

    let plan = plan_auto_pre_split(&eligible(), &stats, config()).expect("plan AUTO split");
    assert_eq!(plan.state, AutoPreSplitPlanState::Planned);
    assert_eq!(
        plan.boundary_rows,
        vec![
            vec![Datum::Null],
            vec![Datum::Int(10)],
            vec![Datum::Int(50)],
            vec![Datum::Int(80)],
        ]
    );
    assert_eq!(
        plan.split_keys.len(),
        4,
        "NULL plus three non-NULL distribution boundaries"
    );
}

#[test]
fn one_hot_value_crossing_multiple_thresholds_is_emitted_once() {
    let stats = DistributionStats {
        stats_version: 2,
        top_n: vec![(Datum::Int(10), 60)],
        histogram: vec![
            HistogramPoint::new(Datum::Int(20), 20),
            HistogramPoint::new(Datum::Int(40), 40),
        ],
        ..Default::default()
    };

    let plan = plan_auto_pre_split(&eligible(), &stats, config()).expect("plan AUTO split");
    assert_eq!(
        plan.boundary_rows,
        vec![vec![Datum::Int(10)], vec![Datum::Int(20)]]
    );
}

#[test]
fn unreliable_statistics_are_skipped_with_specific_reasons() {
    let stats = DistributionStats {
        stats_version: 2,
        top_n: vec![(Datum::Int(1), 100)],
        ..Default::default()
    };
    let cases = [
        (
            AutoPreSplitEligibility {
                partitioned: true,
                ..eligible()
            },
            "partitioned table",
        ),
        (
            AutoPreSplitEligibility {
                partial_index: true,
                ..eligible()
            },
            "partial index",
        ),
        (
            AutoPreSplitEligibility {
                leading_string_prefix: true,
                ..eligible()
            },
            "leading string column uses prefix index",
        ),
        (
            AutoPreSplitEligibility {
                stats_healthy: None,
                ..eligible()
            },
            "stats health unavailable",
        ),
        (
            AutoPreSplitEligibility {
                stats_healthy: Some(79),
                ..eligible()
            },
            "stats health 79 below threshold 80",
        ),
        (
            AutoPreSplitEligibility {
                row_count: 9,
                ..eligible()
            },
            "row count 9 below threshold 10",
        ),
    ];

    for (eligibility, reason) in cases {
        let plan = plan_auto_pre_split(&eligibility, &stats, config()).unwrap();
        assert_eq!(plan.state, AutoPreSplitPlanState::Skipped);
        assert_eq!(plan.skip_reason.as_deref(), Some(reason));
    }

    let legacy = DistributionStats {
        stats_version: 1,
        ..stats
    };
    let plan = plan_auto_pre_split(&eligible(), &legacy, config()).unwrap();
    assert_eq!(
        plan.skip_reason.as_deref(),
        Some("leading column stats version 1 is not Analyze V2")
    );
}

#[test]
fn invalid_distribution_is_an_error_but_missing_distribution_is_a_skip() {
    let invalid = DistributionStats {
        stats_version: 2,
        null_count: -1,
        ..Default::default()
    };
    assert_eq!(
        plan_auto_pre_split(&eligible(), &invalid, config()),
        Err("leading column statistics have negative null count -1".into())
    );

    let missing = DistributionStats {
        stats_version: 2,
        ..Default::default()
    };
    let plan = plan_auto_pre_split(&eligible(), &missing, config()).unwrap();
    assert_eq!(plan.state, AutoPreSplitPlanState::Skipped);
    assert_eq!(
        plan.skip_reason.as_deref(),
        Some("no usable leading column distribution")
    );
}

#[test]
fn manual_policy_overrides_auto_and_auto_failures_are_best_effort() {
    use crate::index_auto_presplit::{PreSplitMode, run_pre_split, select_pre_split_mode};
    use crate::index_presplit::SplitError;

    let mode = select_pre_split_mode(Some(vec![vec![Datum::Int(1)]]), true);
    assert!(matches!(mode, PreSplitMode::Manual(_)));
    let manual = run_pre_split(
        mode,
        || panic!("AUTO planner must not run for a manual policy"),
        |_| Err(SplitError::Split("manual failed".into())),
    );
    assert_eq!(manual, Err(SplitError::Split("manual failed".into())));

    let auto = run_pre_split(
        PreSplitMode::Auto,
        || Err("statistics unavailable".into()),
        |_| panic!("split must not run when AUTO planning fails"),
    );
    assert_eq!(auto, Ok(None));
}

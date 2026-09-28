// Copyright 2026 AsterSQL.

use crate::{PlanKind, PlanNode, RangeRebuildResult, RebuildPlan4CachedPlan, isSafeRange};

fn index_scan(ranges: Vec<&str>) -> PlanNode {
    PlanNode::New(
        1,
        PlanKind::IndexScan {
            table: "t".into(),
            index: "idx".into(),
            ranges: ranges.into_iter().map(str::to_owned).collect(),
        },
        vec![],
    )
}

#[test]
fn empty_rebuilt_range_is_unsafe() {
    let mut plan = index_scan(vec![]);
    assert!(!RebuildPlan4CachedPlan(&mut plan));
}

#[test]
fn limit_arithmetic_is_not_part_of_range_rebuild() {
    let mut plan = PlanNode::New(
        1,
        PlanKind::Limit {
            offset: u64::MAX,
            count: 1,
        },
        vec![],
    );
    assert!(RebuildPlan4CachedPlan(&mut plan));
}

#[test]
fn index_merge_partial_plans_are_rebuilt() {
    let mut plan = PlanNode::New(
        1,
        PlanKind::IndexMergeReader {
            partial_plans: vec![index_scan(vec![])],
            table_plan: Box::new(PlanNode::New(
                2,
                PlanKind::TableScan { table: "t".into() },
                vec![],
            )),
        },
        vec![],
    );
    assert!(!RebuildPlan4CachedPlan(&mut plan));
}

#[test]
fn range_safety_matches_go_invariants() {
    let access = vec!["a = ?".to_owned()];
    let safe = RangeRebuildResult {
        ranges: vec!["[1,1]".into()],
        access_conditions: access.clone(),
        remained_conditions: vec![],
    };
    assert!(isSafeRange(&access, &safe, false, Some(&["[2,2]".into()])));

    let mut unsafe_result = safe.clone();
    unsafe_result.remained_conditions.push("a > 0".into());
    assert!(!isSafeRange(&access, &unsafe_result, false, None));

    unsafe_result = safe.clone();
    unsafe_result.access_conditions.clear();
    assert!(!isSafeRange(&access, &unsafe_result, false, None));

    unsafe_result = safe;
    unsafe_result.ranges = vec!["full".into()];
    assert!(!isSafeRange(
        &access,
        &unsafe_result,
        false,
        Some(&["[2,2]".into()]),
    ));
}

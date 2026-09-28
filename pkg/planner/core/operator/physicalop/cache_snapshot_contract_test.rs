// Copyright 2026 AsterSQL.

use crate::CACHE_SNAPSHOT_PLAN_CONTRACT;

#[test]
fn go_cacheable_plan_inventory_is_complete() {
    let expected = [
        ("Update", "Update"),
        ("Delete", "Delete"),
        ("Insert", "Insert"),
        ("PhysicalTableScan", "PhysicalTableScan"),
        ("PhysicalIndexScan", "PhysicalIndexScan"),
        ("PhysicalSelection", "PhysicalSelection"),
        ("PhysicalProjection", "PhysicalProjection"),
        ("PhysicalTopN", "PhysicalTopN"),
        ("PhysicalLimit", "PhysicalLimit"),
        ("PhysicalStreamAgg", "PhysicalStreamAgg"),
        ("PhysicalHashAgg", "PhysicalHashAgg"),
        ("PhysicalHashJoin", "PhysicalHashJoin"),
        ("PhysicalMergeJoin", "PhysicalMergeJoin"),
        ("PhysicalIndexJoin", "PhysicalIndexJoin"),
        ("PhysicalIndexHashJoin", "PhysicalIndexHashJoin"),
        ("PhysicalIndexReader", "PhysicalIndexReader"),
        ("PhysicalTableReader", "PhysicalTableReader"),
        ("PhysicalIndexMergeReader", "PhysicalIndexMergeReader"),
        ("PhysicalIndexLookUpReader", "PhysicalIndexLookUpReader"),
        ("PhysicalLocalIndexLookUp", "PhysicalLocalIndexLookup"),
        ("BatchPointGetPlan", "BatchPointGetPlan"),
        ("PointGetPlan", "PointGetPlan"),
        ("PhysicalUnionScan", "PhysicalUnionScan"),
        ("PhysicalUnionAll", "PhysicalUnionAll"),
        ("PhysicalTableDual", "PhysicalTableDual"),
    ];

    assert_eq!(CACHE_SNAPSHOT_PLAN_CONTRACT.len(), 25);
    let go_generated_inventory = include_str!("plan_clone_generated.go")
        .lines()
        .filter_map(|line| {
            line.strip_prefix("func (op *").and_then(|rest| {
                rest.strip_suffix(
                    ") CloneForPlanCache(newCtx base.PlanContext) (base.Plan, bool) {",
                )
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        CACHE_SNAPSHOT_PLAN_CONTRACT
            .iter()
            .map(|entry| entry.go_type)
            .collect::<Vec<_>>(),
        go_generated_inventory,
        "Rust cache snapshot inventory drifted from Go generated clone implementations",
    );
    assert_eq!(
        CACHE_SNAPSHOT_PLAN_CONTRACT
            .iter()
            .map(|entry| (entry.go_type, entry.rust_type))
            .collect::<Vec<_>>(),
        expected,
    );
    assert!(
        CACHE_SNAPSHOT_PLAN_CONTRACT
            .iter()
            .all(|entry| !entry.special_clone_fields.is_empty()),
        "every Go clone implementation must document its non-shallow clone contract"
    );
}

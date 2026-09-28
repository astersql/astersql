// Copyright 2026 AsterSQL.

use crate::plan_cost_ver1::{
    GetPlanCostVer1, PlanCostOption, getCardinality, getCost4PhysicalSort,
};
use crate::task::{PlanKind, PlanNode, StatsInfo, TaskType};

fn node(kind: PlanKind, rows: f64, row_size: f64) -> PlanNode {
    let mut plan = PlanNode::new(kind);
    plan.stats = StatsInfo {
        row_count: rows,
        avg_row_size: row_size,
        histogram_row_size: None,
    };
    plan
}

#[test]
fn ver1_zero_cardinality_and_small_sort_match_go_flooring_rules() {
    let option = PlanCostOption::default();
    let plan = node(PlanKind::Sort, 0.0, 100.0);

    assert_eq!(getCardinality(&plan, option.CostFlag), 0.0);
    assert_eq!(getCost4PhysicalSort(&plan, 0.0, &option), 2.4);
}

#[test]
fn ver1_union_all_uses_max_child_cost_and_concurrency_overhead() {
    let option = PlanCostOption::default();
    let left = node(PlanKind::PointGet, 1.0, 1.0);
    let right = node(PlanKind::PointGet, 1.0, 1.0);
    let mut union = node(PlanKind::UnionAll, 2.0, 1.0).with_children(vec![left, right]);

    // Go: max(childCost) + (1 + childCount) * concurrencyFactor.
    assert_eq!(GetPlanCostVer1(&mut union, TaskType::Root, &option), 30.0);
}

#[test]
fn ver1_exchange_receiver_charges_child_rows_without_row_width() {
    let option = PlanCostOption::default();
    let child = node(PlanKind::PointGet, 5.0, 8.0);
    let mut receiver = node(PlanKind::ExchangeReceiver, 99.0, 100.0).with_children(vec![child]);

    // Go: childCost + childCardinality * networkFactor.
    assert_eq!(GetPlanCostVer1(&mut receiver, TaskType::Mpp, &option), 33.0);
}

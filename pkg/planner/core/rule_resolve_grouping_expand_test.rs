// Copyright 2026 AsterSQL.

use crate::rule_aggregation_elimination::LogicalPlan;
use crate::rule_resolve_grouping_expand::ResolveExpand;
use crate::task::{FieldType, PlanKind, PlanNode, TypeCode};

fn field_type() -> FieldType {
    FieldType {
        code: TypeCode::Int,
        flen: 0,
        decimal: 0,
        unsigned: false,
    }
}

fn leaf(columns: usize) -> LogicalPlan {
    LogicalPlan::Node {
        node: PlanNode {
            kind: PlanKind::TableScan,
            schema: vec![field_type(); columns],
            ..PlanNode::default()
        },
        unique_keys: Vec::new(),
        max_one_row: false,
    }
}

fn expand(grouping_sets: Vec<Vec<usize>>) -> LogicalPlan {
    LogicalPlan::Expand {
        child: Box::new(leaf(3)),
        grouping_sets,
        level_projections: Vec::new(),
        schema: vec![field_type(); 5],
    }
}

#[test]
fn optimize_matches_go_changed_contract_and_accepts_empty_grouping_sets() {
    let (resolved, changed) = ResolveExpand
        .Optimize(expand(Vec::new()))
        .expect("Go GenLevelProjections accepts an empty grouping-set list");

    assert!(
        !changed,
        "Go ResolveExpand always reports planChanged=false"
    );
    let LogicalPlan::Expand {
        level_projections, ..
    } = resolved
    else {
        panic!("expected expand")
    };
    assert!(level_projections.is_empty());
}

#[test]
fn unique_grouping_sets_append_gid_without_gpos() {
    let (resolved, changed) = ResolveExpand
        .Optimize(expand(vec![vec![0, 2], vec![0]]))
        .expect("resolve expand");

    assert!(!changed);
    let LogicalPlan::Expand {
        level_projections, ..
    } = resolved
    else {
        panic!("expected expand")
    };
    assert_eq!(level_projections.len(), 2);
    assert_eq!(level_projections[0].len(), 4);
    assert_eq!(level_projections[0][3].name, "grouping_id:3");
    assert_eq!(level_projections[1][1].name, "col_1");
    assert_eq!(level_projections[1][2].name, "null");
    assert_eq!(level_projections[1][2].column, None);
    assert_eq!(level_projections[1][3].name, "grouping_id:1");
}

#[test]
fn duplicate_grouping_sets_append_gpos_for_each_level() {
    let (resolved, _) = ResolveExpand
        .Optimize(expand(vec![vec![0], vec![0]]))
        .expect("resolve expand");

    let LogicalPlan::Expand {
        level_projections, ..
    } = resolved
    else {
        panic!("expected expand")
    };
    assert_eq!(level_projections[0].len(), 5);
    assert_eq!(level_projections[0][3].name, "grouping_id:1");
    assert_eq!(level_projections[0][4].name, "grouping_position:0");
    assert_eq!(level_projections[1][4].name, "grouping_position:1");
}

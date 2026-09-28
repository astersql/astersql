// Copyright 2026 AsterSQL.

use crate::physical_common_plans::{
    Datum, PartitionType, PhysicalExpr, PhysicalProperty, SortItem, Stats, TaskType,
};
use crate::physical_expand::{PhysicalExpand, exhaust_physical_expand};

fn expand() -> PhysicalExpand {
    PhysicalExpand {
        levels: vec![vec![
            PhysicalExpr::Column(1),
            PhysicalExpr::Constant(Datum::Null),
        ]],
        generated_column_names: vec!["grouping_id".into()],
        grouping_ids: vec![1],
        grouping_pos: vec![0],
        schema: vec![1, 2],
        child: None,
    }
}

#[test]
fn ordered_property_allows_sort_enforcer_like_go() {
    let property = PhysicalProperty {
        task_type: TaskType::Root,
        sort_items: vec![SortItem {
            column: 1,
            descending: false,
        }],
        ..PhysicalProperty::default()
    };

    let (plans, can_add_enforcer) = exhaust_physical_expand(expand(), &property, Stats::default());
    assert!(plans.is_empty());
    assert!(!can_add_enforcer);
}

#[test]
fn root_property_enumerates_go_equivalent_child_task_candidates() {
    let property = PhysicalProperty::default();
    let stats = Stats {
        row_count: 12.0,
        version: 7,
    };

    let (plans, can_add_enforcer) = exhaust_physical_expand(expand(), &property, stats.clone());
    assert!(can_add_enforcer);
    assert_eq!(plans.len(), 4);
    assert_eq!(
        plans
            .iter()
            .map(|plan| plan.required_properties[0].task_type)
            .collect::<Vec<_>>(),
        vec![TaskType::Mpp, TaskType::Cop, TaskType::Mpp, TaskType::Root]
    );
    assert!(plans.iter().all(|plan| plan.schema == vec![1, 2]));
    assert!(plans.iter().all(|plan| plan.stats == stats));
}

#[test]
fn mpp_partition_requirement_is_rejected_like_go() {
    let property = PhysicalProperty {
        task_type: TaskType::Mpp,
        partition_type: PartitionType::Hash,
        partition_columns: vec![1],
        ..PhysicalProperty::default()
    };

    let (plans, can_add_enforcer) = exhaust_physical_expand(expand(), &property, Stats::default());
    assert!(plans.is_empty());
    assert!(can_add_enforcer);
}

#[test]
fn protobuf_shape_is_independent_of_non_tiflash_store_selection() {
    let expected = expand().to_pb(TaskType::Mpp).unwrap();
    assert_eq!(expand().to_pb(TaskType::Root).unwrap(), expected);
    assert_eq!(expand().to_pb(TaskType::Cop).unwrap(), expected);
}

#[test]
fn explain_info_matches_go_level_projection_and_schema_shape() {
    assert_eq!(
        expand().explain_info(),
        "level-projection:[Column#1,NULL]; schema: [Column#1,Column#2]"
    );
}

// Copyright 2026 AsterSQL.

use crate::physical_common_plans::{
    CteProducerStatus, PartitionType, PhysicalKind, PhysicalPlanNode, PhysicalProperty, SortItem,
    Stats, TaskType,
};
use crate::physical_sequence::{PhysicalSequence, exhaust_physical_sequence};

fn child(id: i64, schema: Vec<i64>) -> PhysicalPlanNode {
    PhysicalPlanNode {
        id,
        kind: PhysicalKind::Other(format!("child-{id}")),
        schema,
        children: Vec::new(),
        stats: Stats::default(),
        required_properties: Vec::new(),
    }
}

fn sequence(mpp_allowed: bool) -> PhysicalSequence {
    let mut sequence = PhysicalSequence {
        children: vec![child(1, vec![10]), child(2, vec![20, 21])],
        schema: vec![999],
        block_offset: 7,
        mpp_allowed: false,
    };
    sequence.set_mpp_allowed(mpp_allowed);
    sequence
}

#[test]
fn mpp_request_propagates_go_cte_properties_and_main_query_requirement() {
    let property = PhysicalProperty {
        task_type: TaskType::Mpp,
        sort_items: vec![SortItem {
            column: 20,
            descending: true,
        }],
        partition_type: PartitionType::Hash,
        partition_columns: vec![20],
        cte_producer_status: CteProducerStatus::Unknown,
        no_cop_push_down: true,
        ..PhysicalProperty::default()
    };

    let (plans, can_add_enforcer) =
        exhaust_physical_sequence(sequence(true), &property, Stats::default());

    assert!(can_add_enforcer);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].schema, vec![20, 21]);
    assert_eq!(plans[0].required_properties.len(), 2);
    let producer = &plans[0].required_properties[0];
    assert_eq!(producer.task_type, TaskType::Mpp);
    assert_eq!(producer.expected_count, f64::MAX);
    assert_eq!(producer.partition_type, PartitionType::Any);
    assert!(producer.can_add_enforcer);
    assert_eq!(producer.cte_producer_status, CteProducerStatus::AllCanMpp);
    assert!(producer.no_cop_push_down);
    assert_eq!(plans[0].required_properties[1], property);
}

#[test]
fn failed_cte_mpp_status_rejects_mpp_sequence() {
    let property = PhysicalProperty {
        task_type: TaskType::Mpp,
        cte_producer_status: CteProducerStatus::SomeFailedMpp,
        ..PhysicalProperty::default()
    };

    let (plans, can_add_enforcer) =
        exhaust_physical_sequence(sequence(true), &property, Stats::default());
    assert!(plans.is_empty());
    assert!(can_add_enforcer);
}

#[test]
fn unordered_root_request_enumerates_root_and_mpp_choices_like_go() {
    let property = PhysicalProperty {
        no_cop_push_down: true,
        ..PhysicalProperty::default()
    };

    let (plans, _) = exhaust_physical_sequence(sequence(true), &property, Stats::default());

    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|plan| plan.schema == vec![20, 21]));
    assert_eq!(plans[0].required_properties[0].task_type, TaskType::Root);
    assert_eq!(
        plans[0].required_properties[0].cte_producer_status,
        CteProducerStatus::SomeFailedMpp
    );
    assert_eq!(
        plans[0].required_properties[1].cte_producer_status,
        CteProducerStatus::SomeFailedMpp
    );
    assert_eq!(plans[1].required_properties[0].task_type, TaskType::Mpp);
    assert_eq!(plans[1].required_properties[1].task_type, TaskType::Mpp);
    assert!(
        plans[1]
            .required_properties
            .iter()
            .all(|required| required.no_cop_push_down)
    );
}

#[test]
fn ordered_or_mpp_disabled_root_request_does_not_add_mpp_choice() {
    let ordered = PhysicalProperty {
        sort_items: vec![SortItem {
            column: 20,
            descending: false,
        }],
        ..PhysicalProperty::default()
    };
    assert_eq!(
        exhaust_physical_sequence(sequence(true), &ordered, Stats::default())
            .0
            .len(),
        1
    );
    assert_eq!(
        exhaust_physical_sequence(
            sequence(false),
            &PhysicalProperty::default(),
            Stats::default()
        )
        .0
        .len(),
        1
    );
}

#[test]
fn schema_attach_and_explain_follow_the_main_query_like_go() {
    let mut sequence = sequence(true);
    sequence.children[1].stats = Stats {
        row_count: 42.0,
        version: 3,
    };
    assert_eq!(sequence.output_schema(), Some([20, 21].as_slice()));
    assert_eq!(sequence.explain_info(), "Sequence Node");

    let attached = sequence.attach_to_tasks().unwrap();
    assert_eq!(attached.id, 3);
    assert_eq!(attached.schema, vec![20, 21]);
    assert_eq!(attached.stats.row_count, 42.0);
}

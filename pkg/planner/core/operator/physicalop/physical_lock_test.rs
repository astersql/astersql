// Copyright 2026 AsterSQL.

use std::collections::BTreeMap;

use super::physical_common_plans::{
    PartitionType, PhysicalKind, PhysicalPlanNode, PhysicalProperty, SortItem, Stats, TaskType,
};
use super::physical_lock::{LockInfo, PhysicalLock, exhaust_physical_lock};

fn lock(child_schema: Vec<i64>) -> PhysicalLock {
    PhysicalLock {
        lock: LockInfo {
            lock_type: "for update".into(),
            wait_seconds: 7,
        },
        table_id_to_handles: BTreeMap::from([(1, vec![10])]),
        table_id_to_physical_id_column: BTreeMap::from([(1, 99)]),
        child: PhysicalPlanNode {
            id: 41,
            kind: PhysicalKind::Scan { table_id: 1 },
            schema: child_schema,
            children: Vec::new(),
            stats: Stats::default(),
            required_properties: Vec::new(),
        },
    }
}

#[test]
fn enumeration_preserves_go_child_property_for_non_mpp_tasks() {
    let property = PhysicalProperty {
        task_type: TaskType::Cop,
        sort_items: vec![SortItem {
            column: 10,
            descending: true,
        }],
        expected_count: 8.0,
        partition_type: PartitionType::Hash,
        partition_columns: vec![10],
        can_add_enforcer: false,
        ..PhysicalProperty::default()
    };

    let (plans, complete, warnings) =
        exhaust_physical_lock(lock(vec![10, 99]), &property, Stats::default());

    assert!(complete);
    assert!(warnings.is_empty());
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].required_properties, [property]);
}

#[test]
fn resolve_indices_only_resolves_handle_columns_like_go() {
    let plan = lock(vec![10]);

    assert_eq!(plan.resolve_indices(), Ok(()));
}

#[test]
fn mpp_is_rejected_with_the_go_warning() {
    let property = PhysicalProperty {
        task_type: TaskType::Mpp,
        ..PhysicalProperty::default()
    };

    let (plans, complete, warnings) =
        exhaust_physical_lock(lock(vec![10]), &property, Stats::default());

    assert!(complete);
    assert!(plans.is_empty());
    assert_eq!(
        warnings,
        ["MPP mode may be blocked because operator `Lock` is not supported now."]
    );
}

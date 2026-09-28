// Copyright 2026 AsterSQL.

use super::enforce::{EnforceContext, PhysicalTask, enforce_exchanger};
use super::physical_common_plans::{
    PartitionType, PhysicalKind, PhysicalPlanNode, PhysicalProperty, SortItem, Stats, TaskType,
};
use std::collections::BTreeMap;

fn task(partition_type: PartitionType, hash_columns: Vec<i64>) -> PhysicalTask {
    PhysicalTask {
        plan: PhysicalPlanNode {
            id: 7,
            kind: PhysicalKind::Other("input".into()),
            schema: vec![1, 2, 3, 4],
            children: Vec::new(),
            stats: Stats {
                row_count: 12.0,
                version: 3,
            },
            required_properties: Vec::new(),
        },
        task_type: TaskType::Mpp,
        invalid: false,
        partition_type,
        hash_columns,
        warnings: Vec::new(),
    }
}

fn property(partition_type: PartitionType, partition_columns: Vec<i64>) -> PhysicalProperty {
    PhysicalProperty {
        task_type: TaskType::Mpp,
        partition_type,
        partition_columns,
        ..PhysicalProperty::default()
    }
}

#[test]
fn any_partition_never_enforces_exchange() {
    let original = task(PartitionType::Hash, vec![1]);
    let result = enforce_exchanger(
        original.clone(),
        &property(PartitionType::Any, Vec::new()),
        &EnforceContext::default(),
        &BTreeMap::new(),
    );

    assert_eq!(result, original);
}

#[test]
fn broadcast_always_enforces_exchange() {
    let original = task(PartitionType::Broadcast, Vec::new());
    let result = enforce_exchanger(
        original,
        &property(PartitionType::Broadcast, Vec::new()),
        &EnforceContext::default(),
        &BTreeMap::new(),
    );

    assert!(matches!(result.plan.kind, PhysicalKind::ExchangeReceiver));
    assert_eq!(result.partition_type, PartitionType::Broadcast);
}

#[test]
fn single_partition_ignores_stale_hash_columns() {
    let original = task(PartitionType::Single, vec![99]);
    let result = enforce_exchanger(
        original.clone(),
        &property(PartitionType::Single, Vec::new()),
        &EnforceContext::default(),
        &BTreeMap::new(),
    );

    assert_eq!(result, original);
}

#[test]
fn hash_partition_accepts_equivalent_supplied_key_subset() {
    let original = task(PartitionType::Hash, vec![1]);
    let equivalences = BTreeMap::from([(1, 2), (2, 3)]);
    let result = enforce_exchanger(
        original.clone(),
        &property(PartitionType::Hash, vec![3, 4]),
        &EnforceContext::default(),
        &equivalences,
    );

    assert_eq!(result, original);
}

#[test]
fn mpp_sort_must_match_partition_columns() {
    let mut required = property(PartitionType::Hash, vec![1]);
    required.sort_items = vec![SortItem {
        column: 2,
        descending: false,
    }];
    let context = EnforceContext {
        allow_mpp: true,
        ..EnforceContext::default()
    };

    let result = super::enforce::enforce_property(
        &required,
        task(PartitionType::Hash, vec![1]),
        &context,
        &BTreeMap::new(),
    );

    assert!(result.invalid);
    assert_eq!(
        result.warnings,
        vec!["MPP mode may be blocked because operator `Sort` is not supported now."]
    );
}

#[test]
fn new_collation_string_hash_warning_matches_go() {
    let context = EnforceContext {
        new_collation: true,
        string_columns: vec![1],
        ..EnforceContext::default()
    };
    let result = enforce_exchanger(
        task(PartitionType::Single, Vec::new()),
        &property(PartitionType::Hash, vec![1]),
        &context,
        &BTreeMap::new(),
    );

    assert!(result.invalid);
    assert_eq!(
        result.warnings,
        vec![
            "MPP mode may be blocked because when `new_collation_enabled` is true, HashJoin or HashAgg with string key is not supported now."
        ]
    );
}

#[test]
fn exchange_preserves_warnings_and_negotiates_compression_from_v1() {
    let mut original = task(PartitionType::Single, Vec::new());
    original.warnings.push("existing warning".into());
    let context = EnforceContext {
        mpp_version: 1,
        compression: "fast".into(),
        ..EnforceContext::default()
    };
    let result = enforce_exchanger(
        original,
        &property(PartitionType::Hash, vec![1]),
        &context,
        &BTreeMap::new(),
    );

    assert_eq!(result.warnings, vec!["existing warning"]);
    let sender = &result.plan.children[0];
    assert!(matches!(
        &sender.kind,
        PhysicalKind::ExchangeSender { compression, .. } if compression == "fast"
    ));
}

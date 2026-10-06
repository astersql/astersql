// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{BTreeMap, BTreeSet};

use astersql_meta_model as model;
use astersql_parser_ast::NewCIStr;

use crate::storage_class_transition::*;

fn partition(id: i64, name: &str, tier: &str) -> model::PartitionDefinition {
    model::PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        StorageClassTier: tier.to_owned(),
        ..Default::default()
    }
}

fn table() -> model::TableInfo {
    model::TableInfo {
        ID: 10,
        Name: NewCIStr("orders"),
        Partition: Some(model::PartitionInfo {
            Definitions: vec![
                partition(11, "p0", "IA"),
                partition(12, "p1", "IA"),
                partition(13, "p2", "STANDARD"),
            ],
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn operations_group_targets_and_preserve_partition_identity() {
    let table = table();
    let operations = build_operations(
        &table,
        &BTreeSet::from([11, 12, 13]),
        9,
        1234,
        "test",
        "orders",
    )
    .unwrap();
    assert_eq!(operations.len(), 2);
    let ia = operations
        .iter()
        .find(|operation| operation.status.direction == DIRECTION_TO_IA)
        .unwrap();
    assert_eq!(ia.status.physical_table_ids, [11, 12]);
    assert_eq!(ia.status.partition_id, 0);
    let standard = operations
        .iter()
        .find(|operation| operation.status.direction == DIRECTION_TO_STANDARD)
        .unwrap();
    assert_eq!(standard.status.physical_table_ids, [13]);
    assert_eq!(standard.status.partition_id, 13);
    assert_eq!(standard.status.partition_name, "p2");
}

#[test]
fn partitioned_table_parent_is_a_real_physical_target() {
    let mut table = table();
    table.StorageClassTier = "IA".into();
    let operation = build_operations(
        &table,
        &BTreeSet::from([10, 11, 12]),
        9,
        1234,
        "test",
        "orders",
    )
    .unwrap()
    .remove(0);
    assert_eq!(operation.status.physical_table_ids, [10, 11, 12]);
    assert_eq!(operation.status.partition_id, 0);
}

#[test]
fn changed_ids_compare_normalized_tiers_only_for_surviving_targets() {
    let mut table = table();
    let old = snapshot_physical_storage_classes(&table);
    table.Partition.as_mut().unwrap().Definitions[0].StorageClassTier = "STANDARD".into();
    table.Partition.as_mut().unwrap().Definitions.remove(1);
    assert_eq!(
        changed_physical_ids(&old, &snapshot_physical_storage_classes(&table)),
        BTreeSet::from([11])
    );
}

#[test]
fn superseded_operation_restarts_only_targets_that_still_exist() {
    let mut ids = BTreeSet::from([13]);
    let current = BTreeMap::from([
        (
            11,
            PhysicalStorageClass {
                target: Default::default(),
                tier: String::new(),
            },
        ),
        (
            13,
            PhysicalStorageClass {
                target: Default::default(),
                tier: String::new(),
            },
        ),
    ]);
    add_current_targets(
        &mut ids,
        &current,
        &[
            StorageClassTransitionTarget {
                physical_id: 11,
                ..Default::default()
            },
            StorageClassTransitionTarget {
                physical_id: 12,
                ..Default::default()
            },
        ],
    );
    assert_eq!(ids, BTreeSet::from([11, 13]));
}

#[test]
fn progress_requires_an_observation_and_one_fully_ready_sample() {
    let mut operation =
        build_operations(&table(), &BTreeSet::from([11]), 9, 1234, "test", "orders")
            .unwrap()
            .remove(0);
    assert!(!update_progress(&mut operation, 0, 0, false));
    assert!(!operation.status.progress_valid);
    assert!(!update_progress(&mut operation, 1, 2, true));
    assert_eq!(operation.status.progress, 0.5);
    let mut full = operation.clone();
    full.status.completed_replicas = 0;
    full.status.total_replicas = 0;
    full.status.status_valid = false;
    assert!(update_progress(&mut full, 2, 2, true));
    assert_eq!(full.status.progress, 1.0);
}

#[test]
fn target_validation_rejects_empty_zero_and_duplicates() {
    assert!(validate_targets(&[]).is_err());
    assert!(validate_targets(&[Default::default()]).is_err());
    assert!(
        validate_targets(&[
            StorageClassTransitionTarget {
                physical_id: 1,
                ..Default::default()
            },
            StorageClassTransitionTarget {
                physical_id: 1,
                ..Default::default()
            },
        ])
        .is_err()
    );
    assert!(
        validate_targets(&[
            StorageClassTransitionTarget {
                physical_id: 1,
                ..Default::default()
            },
            StorageClassTransitionTarget {
                physical_id: 2,
                ..Default::default()
            },
        ])
        .is_ok()
    );
}

#[test]
fn topology_replacement_respects_parent_shape_target_and_claims() {
    let mut table = table();
    let operation = build_operations(&table, &BTreeSet::from([11, 12]), 9, 1234, "test", "orders")
        .unwrap()
        .remove(0);
    assert!(targets_exist(&table, &operation));
    table.Partition.as_mut().unwrap().Definitions = vec![
        partition(11, "p0", "IA"),
        partition(14, "p1", "IA"),
        partition(13, "p2", "STANDARD"),
    ];
    assert!(!targets_exist(&table, &operation));
    assert_eq!(
        replacement_physical_ids(&table, &operation, &BTreeSet::from([14])),
        BTreeSet::from([11])
    );
    table.Partition.as_mut().unwrap().DDLState = model::StateWriteOnly;
    assert!(!topology_is_stable(&table));
    assert!(schema_published(11, 11));
    assert!(!schema_published(10, 11));
}

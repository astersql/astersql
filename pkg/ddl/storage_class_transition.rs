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

//! Durable storage-class transition model shared by DDL staging and status readers.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use astersql_meta_model as model;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const DIRECTION_TO_IA: &str = "TO_IA";
pub const DIRECTION_TO_STANDARD: &str = "TO_STANDARD";
pub const STATE_RUNNING: &str = "RUNNING";
pub const STATE_COMPLETED: &str = "COMPLETED";
pub const STATE_SUPERSEDED: &str = "SUPERSEDED";

#[derive(Clone, Debug, PartialEq)]
pub struct StorageClassTransitionStatus {
    pub table_schema: String,
    pub table_name: String,
    pub table_id: i64,
    pub partition_name: String,
    pub partition_id: i64,
    pub direction: String,
    pub total_replicas: u64,
    pub completed_replicas: u64,
    pub progress: f64,
    pub progress_valid: bool,
    pub start_time: DateTime<Utc>,
    pub duration: Duration,
    pub last_update_time: Option<DateTime<Utc>>,
    pub status_valid: bool,
    pub physical_table_ids: Vec<i64>,
    pub schema_version: i64,
    pub start_ts: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageClassTransitionTarget {
    pub physical_id: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub partition_id: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub partition_name: String,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalStorageClass {
    pub target: StorageClassTransitionTarget,
    pub tier: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StorageClassTransitionOperation {
    pub status: StorageClassTransitionStatus,
    pub target: String,
    pub targets: Vec<StorageClassTransitionTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StorageClassTransitionKey {
    table_id: i64,
    direction: String,
    start_ts: u64,
}

#[derive(Debug, Default)]
pub struct StorageClassTransitionManager {
    observed: RwLock<BTreeMap<StorageClassTransitionKey, StorageClassTransitionStatus>>,
}

impl StorageClassTransitionOperation {
    fn key(&self) -> StorageClassTransitionKey {
        StorageClassTransitionKey {
            table_id: self.status.table_id,
            direction: self.status.direction.clone(),
            start_ts: self.status.start_ts,
        }
    }
}

fn same_status(left: &StorageClassTransitionStatus, right: &StorageClassTransitionStatus) -> bool {
    left.table_id == right.table_id
        && left.schema_version == right.schema_version
        && left.start_ts == right.start_ts
        && left.partition_id == right.partition_id
        && left.direction == right.direction
        && left.start_time == right.start_time
        && left.physical_table_ids == right.physical_table_ids
}

impl StorageClassTransitionManager {
    pub fn observe(&self, operation: &StorageClassTransitionOperation) {
        self.observed
            .write()
            .expect("storage class transition cache poisoned")
            .insert(operation.key(), operation.status.clone());
    }

    pub fn cached_observation(
        &self,
        operation: &StorageClassTransitionOperation,
    ) -> Option<StorageClassTransitionStatus> {
        self.observed
            .read()
            .expect("storage class transition cache poisoned")
            .get(&operation.key())
            .filter(|status| status.status_valid && same_status(status, &operation.status))
            .cloned()
    }

    pub fn remove(&self, operation: &StorageClassTransitionOperation) {
        self.observed
            .write()
            .expect("storage class transition cache poisoned")
            .remove(&operation.key());
    }

    pub fn retain_active(&self, operations: &[StorageClassTransitionOperation]) {
        let active = operations
            .iter()
            .map(StorageClassTransitionOperation::key)
            .collect::<BTreeSet<_>>();
        self.observed
            .write()
            .expect("storage class transition cache poisoned")
            .retain(|key, _| active.contains(key));
    }

    pub fn clear(&self) {
        self.observed
            .write()
            .expect("storage class transition cache poisoned")
            .clear();
    }
}

pub fn normalized_target(tier: &str) -> &str {
    if tier.is_empty() { "STANDARD" } else { tier }
}

pub fn direction(target: &str) -> Result<&'static str, String> {
    match target {
        "IA" => Ok(DIRECTION_TO_IA),
        "STANDARD" => Ok(DIRECTION_TO_STANDARD),
        _ => Err(format!(
            "invalid storage class transition target {target:?}"
        )),
    }
}

pub fn target_for_direction(direction: &str) -> Result<&'static str, String> {
    match direction {
        DIRECTION_TO_IA => Ok("IA"),
        DIRECTION_TO_STANDARD => Ok("STANDARD"),
        _ => Err(format!(
            "invalid storage class transition direction {direction:?}"
        )),
    }
}

pub fn snapshot_physical_storage_classes(
    table: &model::TableInfo,
) -> BTreeMap<i64, PhysicalStorageClass> {
    let mut physical = BTreeMap::from([(
        table.ID,
        PhysicalStorageClass {
            target: StorageClassTransitionTarget {
                physical_id: table.ID,
                ..Default::default()
            },
            tier: table.StorageClassTier.clone(),
        },
    )]);
    if let Some(partition) = &table.Partition {
        for definition in &partition.Definitions {
            physical.insert(
                definition.ID,
                PhysicalStorageClass {
                    target: StorageClassTransitionTarget {
                        physical_id: definition.ID,
                        partition_id: definition.ID,
                        partition_name: definition.Name.O.clone(),
                    },
                    tier: definition.StorageClassTier.clone(),
                },
            );
        }
    }
    physical
}

pub fn changed_physical_ids(
    old: &BTreeMap<i64, PhysicalStorageClass>,
    current: &BTreeMap<i64, PhysicalStorageClass>,
) -> BTreeSet<i64> {
    current
        .iter()
        .filter_map(|(id, state)| {
            old.get(id)
                .filter(|previous| {
                    normalized_target(&previous.tier) != normalized_target(&state.tier)
                })
                .map(|_| *id)
        })
        .collect()
}

pub fn build_operations(
    table: &model::TableInfo,
    physical_ids: &BTreeSet<i64>,
    schema_version: i64,
    start_ts: u64,
    schema_name: &str,
    table_name: &str,
) -> Result<Vec<StorageClassTransitionOperation>, String> {
    if schema_version <= 0 {
        return Err("storage class transition schema version is unavailable".into());
    }
    if start_ts == 0 {
        return Err("storage class transition start TSO is unavailable".into());
    }
    let physical = snapshot_physical_storage_classes(table);
    let mut by_target: BTreeMap<String, StorageClassTransitionOperation> = BTreeMap::new();
    for physical_id in physical_ids {
        let state = physical.get(physical_id).ok_or_else(|| {
            format!(
                "physical table {physical_id} is missing from table {}",
                table.ID
            )
        })?;
        let target = normalized_target(&state.tier).to_owned();
        let transition_direction = direction(&target)?.to_owned();
        let operation =
            by_target
                .entry(target.clone())
                .or_insert_with(|| StorageClassTransitionOperation {
                    status: StorageClassTransitionStatus {
                        table_schema: schema_name.to_owned(),
                        table_name: table_name.to_owned(),
                        table_id: table.ID,
                        partition_name: String::new(),
                        partition_id: 0,
                        direction: transition_direction,
                        total_replicas: 0,
                        completed_replicas: 0,
                        progress: 0.0,
                        progress_valid: false,
                        start_time: model::TSConvert2Time(start_ts),
                        duration: Duration::zero(),
                        last_update_time: None,
                        status_valid: false,
                        physical_table_ids: Vec::new(),
                        schema_version,
                        start_ts,
                    },
                    target,
                    targets: Vec::new(),
                });
        operation.targets.push(state.target.clone());
    }
    let mut operations = by_target.into_values().collect::<Vec<_>>();
    for operation in &mut operations {
        set_targets(operation);
    }
    operations.sort_by(|left, right| left.status.direction.cmp(&right.status.direction));
    Ok(operations)
}

pub fn set_targets(operation: &mut StorageClassTransitionOperation) {
    operation.targets.sort_by_key(|target| target.physical_id);
    operation.status.physical_table_ids = operation
        .targets
        .iter()
        .map(|target| target.physical_id)
        .collect();
    operation.status.partition_id = 0;
    operation.status.partition_name.clear();
    if operation.targets.len() == 1 && operation.targets[0].partition_id != 0 {
        operation.status.partition_id = operation.targets[0].partition_id;
        operation.status.partition_name = operation.targets[0].partition_name.clone();
    }
}

pub fn validate_targets(targets: &[StorageClassTransitionTarget]) -> Result<(), String> {
    if targets.is_empty() {
        return Err("storage class transition has no physical targets".into());
    }
    let mut seen = BTreeSet::new();
    for target in targets {
        if target.physical_id == 0 {
            return Err("storage class transition has a zero physical table ID".into());
        }
        if !seen.insert(target.physical_id) {
            return Err(format!(
                "duplicate physical table {} in storage class transition",
                target.physical_id
            ));
        }
    }
    Ok(())
}

pub fn targets_exist(
    table: &model::TableInfo,
    operation: &StorageClassTransitionOperation,
) -> bool {
    let physical = snapshot_physical_storage_classes(table);
    operation.targets.iter().all(|target| {
        physical
            .get(&target.physical_id)
            .is_some_and(|current| normalized_target(&current.tier) == operation.target)
    })
}

pub fn replacement_physical_ids(
    table: &model::TableInfo,
    operation: &StorageClassTransitionOperation,
    claimed: &BTreeSet<i64>,
) -> BTreeSet<i64> {
    let tracks_table = operation
        .targets
        .iter()
        .any(|target| target.physical_id == table.ID);
    snapshot_physical_storage_classes(table)
        .into_iter()
        .filter_map(|(physical_id, current)| {
            if (physical_id == table.ID && !tracks_table)
                || claimed.contains(&physical_id)
                || normalized_target(&current.tier) != operation.target
            {
                None
            } else {
                Some(physical_id)
            }
        })
        .collect()
}

pub fn topology_is_stable(table: &model::TableInfo) -> bool {
    table
        .Partition
        .as_ref()
        .is_none_or(|partition| partition.DDLState == model::StateNone)
}

pub fn add_current_targets(
    physical_ids: &mut BTreeSet<i64>,
    current: &BTreeMap<i64, PhysicalStorageClass>,
    targets: &[StorageClassTransitionTarget],
) {
    physical_ids.extend(
        targets
            .iter()
            .filter(|target| current.contains_key(&target.physical_id))
            .map(|target| target.physical_id),
    );
}

pub fn touches(operation: &StorageClassTransitionOperation, ids: &BTreeSet<i64>) -> bool {
    operation
        .targets
        .iter()
        .any(|target| ids.contains(&target.physical_id))
}

pub fn update_progress(
    operation: &mut StorageClassTransitionOperation,
    ready: u64,
    total: u64,
    observed: bool,
) -> bool {
    operation.status.completed_replicas = ready;
    operation.status.total_replicas = total;
    operation.status.status_valid = observed;
    operation.status.progress_valid =
        operation.status.status_valid && operation.status.total_replicas != 0;
    if operation.status.progress_valid {
        operation.status.progress =
            operation.status.completed_replicas as f64 / operation.status.total_replicas as f64;
    }
    observed && total != 0 && ready == total
}

pub fn schema_published(latest: i64, required: i64) -> bool {
    required > 0 && latest >= required
}

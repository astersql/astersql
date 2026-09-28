// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Placement 调度状态累积与策略展示串的单元测试。
//
// `accumulateState` 在多个 range 上取最落后状态；`PlacementSettings::String` 原样返回展示文本。

use std::cell::Cell;
use std::collections::HashMap;

use crate::show_placement::{
    CiString, DatabaseInfo, PartitionDefinition, PlacementScheduleState, PlacementSettings,
    PlacementValue, PolicyInfo, PolicyRefInfo, ShowPlacementBackend, ShowPlacementExec,
    SpecialAttributeDatabase, StoreLabelsJson, TableInfo, accumulateState, fetchTableScheduleState,
};

#[derive(Default)]
struct MockBackend {
    policies: Vec<PolicyInfo>,
    schemas: Vec<DatabaseInfo>,
    special: Vec<SpecialAttributeDatabase>,
    selected: Option<TableInfo>,
    states: HashMap<i64, Result<PlacementScheduleState, String>>,
    replication_calls: Cell<usize>,
    table_lookups: Cell<usize>,
    database_visible: bool,
}

impl ShowPlacementBackend for MockBackend {
    type Context = ();
    type Error = String;

    fn error(&self, message: String) -> Self::Error {
        message
    }
    fn database_access_denied(&self) -> Self::Error {
        "database access denied".into()
    }
    fn database_not_exists(&self, database: &str) -> Self::Error {
        format!("unknown database {database}")
    }
    fn unknown_partition(&self, partition: &str, table: &str) -> Self::Error {
        format!("unknown partition {partition} in {table}")
    }
    fn restricted_store_labels(&self, _: &()) -> Result<Vec<StoreLabelsJson>, String> {
        Ok(Vec::new())
    }
    fn database_visibility_check_enabled(&self) -> bool {
        true
    }
    fn database_is_visible(&self, _: &str) -> bool {
        self.database_visible
    }
    fn table_privilege_check_enabled(&self) -> bool {
        false
    }
    fn table_is_visible(&self, _: &str, _: &str) -> bool {
        true
    }
    fn schema_by_name(&self, name: &CiString) -> Option<DatabaseInfo> {
        self.schemas
            .iter()
            .find(|db| db.name.lower == name.lower)
            .cloned()
    }
    fn selected_table(&self, _: &CiString, _: &CiString) -> Result<TableInfo, String> {
        self.selected
            .clone()
            .ok_or_else(|| "table not found".into())
    }
    fn all_placement_policies(&self) -> Vec<PolicyInfo> {
        self.policies.clone()
    }
    fn all_schema_names(&self) -> Vec<CiString> {
        self.schemas.iter().map(|db| db.name.clone()).collect()
    }
    fn schema_simple_table_ids(&self, _: &(), _: &CiString) -> Result<Vec<i64>, String> {
        Ok(self
            .special
            .iter()
            .flat_map(|db| db.table_infos.iter().map(|t| t.id))
            .collect())
    }
    fn tables_with_special_attributes(&self) -> Vec<SpecialAttributeDatabase> {
        self.special.clone()
    }
    fn policy_by_name(&self, name: &CiString) -> Option<PolicyInfo> {
        self.policies
            .iter()
            .find(|policy| policy.name.lower == name.lower)
            .cloned()
    }
    fn table_by_id(&self, _: &(), id: i64) -> Option<TableInfo> {
        self.table_lookups.set(self.table_lookups.get() + 1);
        self.special
            .iter()
            .flat_map(|db| &db.table_infos)
            .find(|table| table.id == id)
            .cloned()
    }
    fn range_policy_name(&self, _: &(), _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn range_key_hex(&self, _: &str) -> (String, String) {
        (String::new(), String::new())
    }
    fn encoded_table_range(&self, id: i64) -> (Vec<u8>, Vec<u8>) {
        (id.to_be_bytes().to_vec(), (id + 1).to_be_bytes().to_vec())
    }
    fn replication_state(
        &self,
        _: &(),
        start: &[u8],
        _: &[u8],
    ) -> Result<PlacementScheduleState, String> {
        self.replication_calls.set(self.replication_calls.get() + 1);
        let id = i64::from_be_bytes(start.try_into().expect("encoded table ID"));
        self.states
            .get(&id)
            .cloned()
            .unwrap_or(Ok(PlacementScheduleState::Scheduled))
    }
}

fn policy(name: &str, display: &str) -> PolicyInfo {
    PolicyInfo {
        name: CiString::new(name),
        placement_settings: PlacementSettings {
            display: display.into(),
        },
    }
}

fn policy_ref(name: &str) -> Option<PolicyRefInfo> {
    Some(PolicyRefInfo {
        name: CiString::new(name),
    })
}

fn strings(row: &[PlacementValue]) -> Vec<String> {
    row.iter()
        .map(|value| match value {
            PlacementValue::String(value) => value.clone(),
            PlacementValue::JsonStringArray(values) => format!("{values:?}"),
        })
        .collect()
}

#[test]
/// Scheduled 与 InProgress 合并得 InProgress；Pending 优先于 Scheduled。
fn placement_state_keeps_the_least_advanced_schedule_state() {
    assert_eq!(
        accumulateState(
            PlacementScheduleState::Scheduled,
            PlacementScheduleState::InProgress,
        ),
        PlacementScheduleState::InProgress
    );
    assert_eq!(
        accumulateState(
            PlacementScheduleState::Pending,
            PlacementScheduleState::Scheduled,
        ),
        PlacementScheduleState::Pending
    );
    assert_eq!(
        PlacementSettings {
            display: "PRIMARY_REGION=\"us-east-1\"".into()
        }
        .String(),
        "PRIMARY_REGION=\"us-east-1\""
    );
}

#[test]
fn show_placement_sorts_policies_and_inherits_table_policy_for_partitions() {
    let table = TableInfo {
        id: 10,
        name: CiString::new("t`z"),
        placement_policy_ref: policy_ref("parent"),
        partitions: Some(vec![
            PartitionDefinition {
                id: 11,
                name: CiString::new("p0"),
                placement_policy_ref: None,
            },
            PartitionDefinition {
                id: 12,
                name: CiString::new("p1"),
                placement_policy_ref: policy_ref("child"),
            },
        ]),
    };
    let backend = MockBackend {
        policies: vec![
            policy("parent", "FOLLOWERS=3"),
            policy("child", "FOLLOWERS=1"),
        ],
        special: vec![SpecialAttributeDatabase {
            database_name: CiString::new("d`b"),
            table_infos: vec![table],
        }],
        database_visible: true,
        ..Default::default()
    };
    let mut exec = ShowPlacementExec {
        backend,
        DBName: CiString::new(""),
        TableSchema: CiString::new(""),
        TableName: CiString::new(""),
        Partition: CiString::new(""),
        rows: Vec::new(),
    };

    exec.fetchShowPlacement(&()).unwrap();

    let rows: Vec<_> = exec.rows.iter().map(|row| strings(row)).collect();
    assert_eq!(rows[0], ["POLICY child", "FOLLOWERS=1", "NULL"]);
    assert_eq!(rows[1], ["POLICY parent", "FOLLOWERS=3", "NULL"]);
    assert_eq!(rows[2], ["TABLE `d``b`.`t``z`", "FOLLOWERS=3", "SCHEDULED"]);
    assert_eq!(
        rows[3],
        [
            "TABLE `d``b`.`t``z` PARTITION p0",
            "FOLLOWERS=3",
            "SCHEDULED"
        ]
    );
    assert_eq!(
        rows[4],
        [
            "TABLE `d``b`.`t``z` PARTITION p1",
            "FOLLOWERS=1",
            "SCHEDULED"
        ]
    );
    // Table and partition IDs are shared through the same cache between rows.
    assert_eq!(exec.backend.replication_calls.get(), 3);
}

#[test]
fn table_schedule_short_circuits_and_preserves_go_error_state() {
    let table = TableInfo {
        id: 20,
        name: CiString::new("t"),
        placement_policy_ref: None,
        partitions: Some(vec![PartitionDefinition {
            id: 21,
            name: CiString::new("p"),
            placement_policy_ref: None,
        }]),
    };
    let backend = MockBackend {
        states: HashMap::from([(20, Ok(PlacementScheduleState::Pending))]),
        ..Default::default()
    };
    assert_eq!(
        fetchTableScheduleState(&backend, &(), None, &table).unwrap(),
        PlacementScheduleState::Pending
    );
    assert_eq!(
        backend.replication_calls.get(),
        1,
        "pending table skips partitions"
    );

    let backend = MockBackend {
        states: HashMap::from([
            (20, Ok(PlacementScheduleState::Scheduled)),
            (21, Err("pd unavailable".into())),
        ]),
        ..Default::default()
    };
    let error = fetchTableScheduleState(&backend, &(), None, &table).unwrap_err();
    assert_eq!(error.state, PlacementScheduleState::Pending);
    assert_eq!(error.error, "pd unavailable");
}

#[test]
fn database_visibility_and_missing_policy_errors_match_go_paths() {
    let database = DatabaseInfo {
        name: CiString::new("private"),
        placement_policy_ref: policy_ref("missing"),
    };
    let backend = MockBackend {
        schemas: vec![database],
        database_visible: false,
        ..Default::default()
    };
    let mut exec = ShowPlacementExec {
        backend,
        DBName: CiString::new("private"),
        TableSchema: CiString::new(""),
        TableName: CiString::new(""),
        Partition: CiString::new(""),
        rows: Vec::new(),
    };
    assert_eq!(
        exec.fetchShowPlacementForDB(&()).unwrap_err(),
        "database access denied"
    );

    exec.backend.database_visible = true;
    assert_eq!(
        exec.fetchShowPlacementForDB(&()).unwrap_err(),
        "Policy with name 'missing' not found"
    );
    assert!(exec.rows.is_empty());
}

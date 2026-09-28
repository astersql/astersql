// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Go `placement_sql_test.go` 的可执行 Rust 对抗测试。

use std::time::Duration;

use crate::executor::{
    ColumnInfo, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist, SessionContext,
    TableInfo,
};
use crate::placement_policy::{
    PlacementObject, PlacementPolicyCatalog, PlacementSettings, PolicyError, PolicyInfo, PolicyRef,
    PolicyState, handle_table_placement,
};

fn executor() -> (Executor<MemoryJobBackend>, SessionContext) {
    (
        Executor::new(MemoryJobBackend::default(), Duration::ZERO),
        SessionContext::default(),
    )
}

fn policy(id: i64, name: &str) -> PolicyInfo {
    PolicyInfo {
        id,
        name: name.into(),
        state: PolicyState::None,
        settings: PlacementSettings {
            primary_region: "r1".into(),
            regions: "r1,r2".into(),
            followers: 3,
            ..PlacementSettings::default()
        },
    }
}

fn table(name: &str) -> TableInfo {
    TableInfo::new(name, vec![ColumnInfo::integer("id")])
}

#[test]
fn create_schema_with_placement_is_visible_and_inherited_by_new_tables() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(
        &mut session,
        "SchemaPolicyPlacementTest",
        &[],
        Some("PolicySchemaTest".into()),
        OnExist::Error,
    )
    .unwrap();
    let mut inherited = table("UseSchemaDefault");
    inherited.placement_policy = ddl.schemas["schemapolicyplacementtest"]
        .placement_policy
        .clone();
    ddl.create_table(
        &mut session,
        "SchemaPolicyPlacementTest",
        inherited,
        OnExist::Error,
    )
    .unwrap();
    let mut overridden = table("UsePolicy");
    overridden.placement_policy = Some("PolicyTableTest".into());
    ddl.create_table(
        &mut session,
        "SchemaPolicyPlacementTest",
        overridden,
        OnExist::Error,
    )
    .unwrap();

    let schema = &ddl.schemas["schemapolicyplacementtest"];
    assert_eq!(Some("PolicySchemaTest"), schema.placement_policy.as_deref());
    assert_eq!(
        Some("PolicySchemaTest"),
        schema.tables["useschemadefault"]
            .placement_policy
            .as_deref()
    );
    assert_eq!(
        Some("PolicyTableTest"),
        schema.tables["usepolicy"].placement_policy.as_deref()
    );
}

#[test]
fn alter_database_placement_changes_only_future_inheritance() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "TestAlterDB", &[], None, OnExist::Error)
        .unwrap();
    ddl.alter_schema_placement(&mut session, "TestAlterDB", Some("alter_x".into()), false)
        .unwrap();
    let mut old_table = table("t");
    old_table.placement_policy = ddl.schemas["testalterdb"].placement_policy.clone();
    ddl.create_table(&mut session, "TestAlterDB", old_table, OnExist::Error)
        .unwrap();
    ddl.alter_schema_placement(&mut session, "TestAlterDB", Some("alter_y".into()), false)
        .unwrap();
    let mut new_table = table("t2");
    new_table.placement_policy = ddl.schemas["testalterdb"].placement_policy.clone();
    ddl.create_table(&mut session, "TestAlterDB", new_table, OnExist::Error)
        .unwrap();

    let schema = &ddl.schemas["testalterdb"];
    assert_eq!(Some("alter_y"), schema.placement_policy.as_deref());
    assert_eq!(
        Some("alter_x"),
        schema.tables["t"].placement_policy.as_deref()
    );
    assert_eq!(
        Some("alter_y"),
        schema.tables["t2"].placement_policy.as_deref()
    );
    ddl.alter_schema_placement(&mut session, "TestAlterDB", Some("default".into()), false)
        .unwrap();
    assert!(ddl.schemas["testalterdb"].placement_policy.is_none());
}

#[test]
fn placement_ignore_mode_warns_and_removes_table_and_partition_refs() {
    let mut catalog = PlacementPolicyCatalog::default();
    catalog.create(policy(1, "p1"), false).unwrap();
    catalog.create(policy(2, "p2"), false).unwrap();
    let mut placed = PlacementObject {
        id: 10,
        policy_ref: Some(PolicyRef {
            id: 1,
            name: "p1".into(),
        }),
        partitions: vec![PlacementObject {
            id: 11,
            policy_ref: Some(PolicyRef {
                id: 2,
                name: "p2".into(),
            }),
            partitions: vec![],
        }],
    };
    assert!(handle_table_placement(&mut placed, &catalog, true).unwrap());
    assert!(placed.policy_ref.is_none());
    assert!(placed.partitions[0].policy_ref.is_none());

    // IGNORE 在解析不存在的策略前短路，对应 Go 的 pxxx/pyyy 场景。
    placed.policy_ref = Some(PolicyRef {
        id: 0,
        name: "pxxx".into(),
    });
    placed.partitions[0].policy_ref = Some(PolicyRef {
        id: 0,
        name: "pyyy".into(),
    });
    assert!(handle_table_placement(&mut placed, &catalog, true).unwrap());
    assert_eq!(
        Err(PolicyError::NotFound),
        catalog.normalize_ref(Some(PolicyRef {
            id: 0,
            name: "pxxx".into()
        }))
    );

    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "db2", &[], None, OnExist::Error)
        .unwrap();
    ddl.alter_schema_placement(&mut session, "db2", Some("pxxx".into()), true)
        .unwrap();
    ddl.alter_schema_charset(&mut session, "db2", "ascii", "ascii_bin")
        .unwrap();
    assert!(ddl.schemas["db2"].placement_policy.is_none());
    assert_eq!("ascii", ddl.schemas["db2"].charset);
    assert_eq!(vec!["placement is ignored"], session.notes);
}

#[test]
fn placement_and_tiflash_replica_coexist_without_overwriting_each_other() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let mut partitioned = table("tp");
    partitioned.placement_policy = Some("p1".into());
    ddl.create_table(&mut session, "test", partitioned, OnExist::Error)
        .unwrap();
    let ident = Ident::new("test", "tp");

    ddl.set_tiflash_replica(&mut session, &ident, 1, 1).unwrap();
    assert_eq!(1, ddl.schemas["test"].tables["tp"].tiflash_replica_count);
    assert_eq!(
        Some("p1"),
        ddl.schemas["test"].tables["tp"].placement_policy.as_deref()
    );
    ddl.set_table_options(
        &mut session,
        &ident,
        None,
        Some("p2".into()),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(1, ddl.schemas["test"].tables["tp"].tiflash_replica_count);
    assert_eq!(
        Some("p2"),
        ddl.schemas["test"].tables["tp"].placement_policy.as_deref()
    );

    assert!(matches!(
        ddl.set_tiflash_replica(&mut session, &ident, 2, 1),
        Err(ExecutorError::Unsupported(message)) if message == "TiFlash replica count exceeds stores"
    ));
    assert_eq!(1, ddl.schemas["test"].tables["tp"].tiflash_replica_count);
    ddl.set_tiflash_replica(&mut session, &ident, 0, 1).unwrap();
    assert_eq!(0, ddl.schemas["test"].tables["tp"].tiflash_replica_count);
    assert_eq!(
        Some("p2"),
        ddl.schemas["test"].tables["tp"].placement_policy.as_deref()
    );
}

#[test]
fn every_go_test_and_helper_has_an_executable_rust_mapping() {
    let go = include_str!("placement_sql_test.go");
    for symbol in [
        "func TestCreateSchemaWithPlacement(",
        "func TestAlterDBPlacement(",
        "func TestPlacementMode(",
        "func checkTiflashReplicaSet(",
        "func TestPlacementTiflashCheck(",
        "func getClonedTableFromDomain(",
        "func getClonedDatabaseFromDomain(",
    ] {
        assert!(go.contains(symbol), "missing Go mapping: {symbol}");
    }
}

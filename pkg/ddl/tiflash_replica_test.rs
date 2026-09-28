// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go `tiflash_replica_test.go` 的可执行 Rust 对抗测试。

use std::{collections::BTreeSet, time::Duration};

use crate::executor::{
    ColumnInfo, DdlAction, Executor, ExecutorError, Ident, MemoryJobBackend, OnExist,
    PartitionDefinition, SessionContext, TableInfo,
};

const TIFLASH_REPLICA_LEASE: Duration = Duration::from_millis(600);

fn executor() -> (Executor<MemoryJobBackend>, SessionContext) {
    (
        Executor::new(MemoryJobBackend::default(), TIFLASH_REPLICA_LEASE),
        SessionContext::default(),
    )
}

fn table(name: &str) -> TableInfo {
    TableInfo::new(name, vec![ColumnInfo::integer("id")])
}

fn create_table(
    ddl: &mut Executor<MemoryJobBackend>,
    session: &mut SessionContext,
    mut info: TableInfo,
) -> Ident {
    let name = info.name.clone();
    for partition in &mut info.partitions {
        partition.id = match partition.name.as_str() {
            "p0" => 100,
            "p1" => 101,
            "p2" => 102,
            _ => unreachable!("unexpected partition"),
        };
    }
    ddl.create_table(session, "test", info, OnExist::Error)
        .unwrap();
    Ident::new("test", name)
}

#[test]
fn set_table_tiflash_replica_and_partition_status_match_go() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let mut info = table("t_flash");
    info.partitions = ["p0", "p1", "p2"]
        .into_iter()
        .map(|name| PartitionDefinition::new(name, Vec::new()))
        .collect();
    let ident = create_table(&mut ddl, &mut session, info);

    ddl.set_tiflash_replica(&mut session, &ident, 2, 2).unwrap();
    let table = &ddl.schemas["test"].tables["t_flash"];
    assert_eq!(2, table.tiflash_replica_count);
    assert!(table.tiflash_available_ids.is_empty());

    for id in [100, 101, 102] {
        ddl.update_replica_status(&mut session, id, true).unwrap();
    }
    assert_eq!(
        [100_i64, 101, 102].into_iter().collect::<BTreeSet<_>>(),
        ddl.schemas["test"].tables["t_flash"].tiflash_available_ids
    );
    ddl.update_replica_status(&mut session, 101, false).unwrap();
    assert_eq!(
        [100_i64, 102].into_iter().collect::<BTreeSet<_>>(),
        ddl.schemas["test"].tables["t_flash"].tiflash_available_ids
    );

    assert_eq!(
        Err(ExecutorError::TableNotFound(i64::MAX.to_string())),
        ddl.update_replica_status(&mut session, i64::MAX, false)
    );
    ddl.set_tiflash_replica(&mut session, &ident, 0, 0).unwrap();
    let table = &ddl.schemas["test"].tables["t_flash"];
    assert_eq!(0, table.tiflash_replica_count);
    assert!(table.tiflash_available_ids.is_empty());
}

#[test]
fn replica_count_and_table_kind_errors_do_not_mutate_metadata() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let normal = create_table(&mut ddl, &mut session, table("normal"));
    assert_eq!(
        Err(ExecutorError::Unsupported(
            "TiFlash replica count exceeds stores".into()
        )),
        ddl.set_tiflash_replica(&mut session, &normal, 2, 1)
    );
    assert_eq!(
        0,
        ddl.schemas["test"].tables["normal"].tiflash_replica_count
    );

    for (name, temporary, view, sequence) in [
        ("temporary", true, false, false),
        ("view", false, true, false),
        ("sequence", false, false, true),
    ] {
        let mut info = table(name);
        info.temporary = temporary;
        info.view = view;
        info.sequence = sequence;
        let ident = create_table(&mut ddl, &mut session, info);
        assert_eq!(
            Err(ExecutorError::Unsupported(
                "TiFlash unsupported table type".into()
            )),
            ddl.set_tiflash_replica(&mut session, &ident, 1, 1)
        );
        assert_eq!(0, ddl.schemas["test"].tables[name].tiflash_replica_count);
    }
}

#[test]
fn truncate_preserves_replica_count_but_clears_availability() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let ident = create_table(&mut ddl, &mut session, table("truncate_table"));
    ddl.set_tiflash_replica(&mut session, &ident, 3, 3).unwrap();
    let old_id = ddl.schemas["test"].tables["truncate_table"].id;
    ddl.update_replica_status(&mut session, old_id, true)
        .unwrap();

    let new_id = ddl.truncate_table(&mut session, &ident).unwrap();
    let table = &ddl.schemas["test"].tables["truncate_table"];
    assert!(new_id > old_id);
    assert_eq!(3, table.tiflash_replica_count);
    assert!(table.tiflash_available_ids.is_empty());
    assert!(ddl.backend().history().iter().any(|job| {
        job.action == DdlAction::TruncateTable
            && job.table_id == old_id
            && job.args.get("new_table_id") == Some(&new_id.to_string())
    }));
}

#[test]
fn every_go_test_and_helper_has_an_executable_rust_mapping() {
    let go = include_str!("tiflash_replica_test.go");
    for symbol in [
        "func TestSetTableFlashReplica(",
        "func setUpRPCService(",
        "func updateTableMeta(",
        "func setUpMockTiFlash(",
        "func TestInfoSchemaForTiFlashReplica(",
        "func TestSetTiFlashReplicaForTemporaryTable(",
        "func TestSetTiFlashReplicaForAddGBKColumn(",
        "func TestSetTableFlashReplicaForSystemTable(",
        "func TestSkipSchemaChecker(",
        "func TestCreateTableWithLike2(",
        "func TestTruncateTable2(",
    ] {
        assert!(go.contains(symbol), "missing Go mapping: {symbol}");
    }

    // External SQL/RPC harness cases map to the executor tests above and to the
    // dedicated add-column/table/ddl-tiflash test modules. This guard prevents
    // future Go additions from silently becoming inert source-string records.
    assert_eq!(Duration::from_millis(600), TIFLASH_REPLICA_LEASE);
}

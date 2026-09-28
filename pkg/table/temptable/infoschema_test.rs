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

use std::sync::Arc;

use crate::{
    CiString, DbInfo, InfoSchema, MemoryInfoSchema, SessionExtendedInfoSchema, SessionTables,
    SessionVariables, SessionVarsProvider, Table, TableInfo,
    attach_local_temporary_table_info_schema, detach_local_temporary_table_info_schema,
};

#[derive(Default)]
struct TestSession {
    variables: Arc<SessionVariables>,
}

impl SessionVarsProvider for TestSession {
    fn session_variables(&self) -> Arc<SessionVariables> {
        Arc::clone(&self.variables)
    }
}

fn table(id: i64, db_id: i64, name: &str) -> Arc<Table> {
    Arc::new(Table::from_metadata(TableInfo {
        id,
        db_id,
        name: CiString::new(name),
        ..TableInfo::default()
    }))
}

#[test]
fn ci_string_uses_unicode_lowercase_like_go_cistr() {
    assert_eq!(CiString::new("ÄSTERSQL").lower(), "ästersql");
}

#[test]
#[should_panic]
fn adding_a_table_with_a_different_database_id_violates_the_go_invariant() {
    SessionTables::new()
        .add_table(
            Arc::new(DbInfo {
                id: 7,
                name: CiString::new("test"),
            }),
            table(11, 8, "t"),
        )
        .unwrap();
}

#[test]
fn removing_a_schemas_last_table_removes_the_retained_schema() {
    let tables = SessionTables::new();
    let db = Arc::new(DbInfo {
        id: 7,
        name: CiString::new("test"),
    });
    tables
        .add_table(Arc::clone(&db), table(11, 7, "t"))
        .unwrap();

    assert!(tables.schema_by_id(7).is_some());
    assert!(tables.remove_table(&db.name, &CiString::new("t")).is_some());
    assert!(tables.schema_by_id(7).is_none());
}

#[test]
fn first_reattach_replaces_the_initial_local_tables_then_once_freezes_it() {
    let first = Arc::new(SessionTables::new());
    first
        .add_table(
            Arc::new(DbInfo {
                id: 1,
                name: CiString::new("db"),
            }),
            table(101, 1, "first"),
        )
        .unwrap();
    let base: Arc<dyn InfoSchema> = Arc::new(MemoryInfoSchema::default());
    let extended: Arc<dyn InfoSchema> =
        Arc::new(SessionExtendedInfoSchema::new(base, Arc::clone(&first)));

    let second_session = TestSession::default();
    let second = Arc::new(SessionTables::new());
    second
        .add_table(
            Arc::new(DbInfo {
                id: 2,
                name: CiString::new("db"),
            }),
            table(202, 2, "second"),
        )
        .unwrap();
    *second_session
        .variables
        .local_temporary_tables
        .lock()
        .unwrap() = Some(Arc::clone(&second));

    let extended = attach_local_temporary_table_info_schema(&second_session, extended);
    assert!(extended.table_by_id(101).is_none());
    assert!(extended.table_by_id(202).is_some());

    let third_session = TestSession::default();
    let third = Arc::new(SessionTables::new());
    third
        .add_table(
            Arc::new(DbInfo {
                id: 3,
                name: CiString::new("db"),
            }),
            table(303, 3, "third"),
        )
        .unwrap();
    *third_session
        .variables
        .local_temporary_tables
        .lock()
        .unwrap() = Some(third);

    let extended = attach_local_temporary_table_info_schema(&third_session, extended);
    assert!(extended.table_by_id(202).is_some());
    assert!(extended.table_by_id(303).is_none());
}

#[test]
fn detach_preserves_the_go_extended_info_schema_shape_without_local_tables() {
    let base = Arc::new(MemoryInfoSchema::default());
    base.insert(table(404, 4, "base"));
    let local = Arc::new(SessionTables::new());
    local
        .add_table(
            Arc::new(DbInfo {
                id: 5,
                name: CiString::new("db"),
            }),
            table(505, 5, "local"),
        )
        .unwrap();
    let attached: Arc<dyn InfoSchema> = Arc::new(SessionExtendedInfoSchema::new(base, local));

    let detached = detach_local_temporary_table_info_schema(attached);
    assert!(detached.as_any().is::<SessionExtendedInfoSchema>());
    assert!(detached.table_by_id(404).is_some());
    assert!(detached.table_by_id(505).is_none());
}

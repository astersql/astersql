// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::{
    CancellationToken, DDLAllSchemaVersions, DatabaseTables, EtcdClient, InMemoryInfoSchema,
    NewDeadTableLockChecker, SessionInfo, TableInfo, TableLockInfo,
};

#[test]
fn exact_schema_version_prefix_key_matches_go_trim_prefix_semantics() {
    let etcd_client = Arc::new(EtcdClient::default());
    etcd_client.put(DDLAllSchemaVersions, "1").unwrap();

    let session = SessionInfo {
        server_id: DDLAllSchemaVersions.to_owned(),
        session_id: 1,
    };
    let info_schema = InMemoryInfoSchema {
        databases: vec![DatabaseTables {
            table_infos: vec![TableInfo {
                id: 2,
                db_id: 3,
                name: "locked_table".to_owned(),
                lock: Some(TableLockInfo {
                    sessions: vec![session.clone()],
                    lock_type: "write".to_owned(),
                }),
            }],
        }],
    };

    let dead_locks = NewDeadTableLockChecker(Some(etcd_client))
        .GetDeadLockedTables(&CancellationToken::default(), &info_schema)
        .unwrap();

    assert!(
        !dead_locks.contains_key(&session),
        "Go strings.TrimPrefix keeps a key that does not start with prefix plus slash"
    );
}

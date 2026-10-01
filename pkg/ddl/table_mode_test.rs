// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 表模式（Table Mode）相关测试。
//
// 可执行部分覆盖模式切换、版本递增与 Import/Restore 下的操作权限矩阵。

/*

// get_cloned_table_info_from_domain 对应 Go helper：从 InfoSchema 读取表并 Clone 元信息。
fn get_cloned_table_info_from_domain(
    t: testing::T,
    db_name: &str,
    table_name: &str,
    dom: &domain::Domain,
) -> *mut model::TableInfo {
    let tbl = dom
        .InfoSchema()
        .TableByName(context::Background(), ast::NewCIStr(db_name), ast::NewCIStr(table_name))
        .expect("Go require.NoError");
    tbl.Meta().Clone()
}
*/

use crate::table_mode::{
    TableInfo, TableMode, TableOperation, alter_table_mode, on_alter_table_mode, table_mode_allows,
};

/// 校验模式切换时 ID 匹配、幂等不升版本，以及 Import↔Restore 非法互转。
#[test]
fn table_mode_transitions_validate_ids_and_advance_version_once() {
    let mut table = TableInfo {
        schema_id: 7,
        table_id: 11,
        mode: TableMode::Normal,
        version: 5,
    };
    // Normal -> Import：版本 5→6。
    assert_eq!(
        6,
        on_alter_table_mode(&mut table, 7, 11, TableMode::Import).unwrap()
    );
    // 幂等：Go 的 onAlterTableMode 不更新 schema，返回零值版本，表版本保持 6。
    assert_eq!(
        0,
        on_alter_table_mode(&mut table, 7, 11, TableMode::Import).unwrap()
    );
    assert_eq!(6, table.version);
    // table_id 不匹配应失败。
    assert!(on_alter_table_mode(&mut table, 7, 12, TableMode::Normal).is_err());
    // Import 不能直接切到 Restore。
    assert!(alter_table_mode(&mut table, TableMode::Restore).is_err());
    // 先回 Normal，再进 Restore；随后 Restore→Import 仍非法。
    assert!(alter_table_mode(&mut table, TableMode::Normal).unwrap());
    assert!(alter_table_mode(&mut table, TableMode::Restore).unwrap());
    assert!(alter_table_mode(&mut table, TableMode::Import).is_err());
}

/// 校验 Import/Restore 仅允许 Metadata/Checksum，Normal 允许全部操作。
#[test]
fn protected_table_modes_allow_metadata_and_checksum_only() {
    for mode in [TableMode::Import, TableMode::Restore] {
        assert!(table_mode_allows(mode, TableOperation::Metadata));
        assert!(table_mode_allows(mode, TableOperation::Checksum));
        for operation in [
            TableOperation::Read,
            TableOperation::Write,
            TableOperation::Alter,
            TableOperation::Drop,
        ] {
            assert!(!table_mode_allows(mode, operation));
        }
    }
    for operation in [
        TableOperation::Metadata,
        TableOperation::Checksum,
        TableOperation::Read,
        TableOperation::Write,
        TableOperation::Alter,
        TableOperation::Drop,
    ] {
        assert!(table_mode_allows(TableMode::Normal, operation));
    }
}

#[test]
fn crossks_align_normal_ddl_real_meta_go_diff_and_stage_cleanup() {
    use astersql_kv::Storage;
    use astersql_meta_model::group_3::{Job, JobState, JobVersion};
    use astersql_meta_model::{DBInfo, SchemaState, TableInfo as FullTable, TableMode as FullMode};
    use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage};
    let store = NewMockStorage(KVStore::NewMemory(), None).unwrap();
    let mut txn = astersql_kv::Storage::Begin(store.as_ref(), &[]).unwrap();
    let db = DBInfo {
        ID: 11,
        State: SchemaState::Public,
        ..Default::default()
    };
    let table = FullTable {
        ID: 22,
        State: SchemaState::Public,
        Comment: "preserve metadata".repeat(300),
        ..Default::default()
    };
    txn.Set(
        astersql_meta::transaction_meta_hash_key(b"DBs", b"DB:11"),
        astersql_meta_model::EncodeDBInfo(&db).unwrap(),
    )
    .unwrap();
    txn.Set(
        astersql_meta::transaction_meta_hash_key(b"DB:11", b"Table:22"),
        astersql_meta_model::EncodeTableInfo(&table).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    // Use Go V1 args, retained verbatim across action/history encoding.
    let mut job = Job {
        id: 33,
        tp: 75,
        schema_id: 11,
        table_id: 22,
        version: JobVersion::V1,
        raw_args: br#"[{"table_mode":1}]"#.to_vec(),
        ..Default::default()
    };
    let mut txn = astersql_kv::Storage::Begin(store.as_ref(), &[]).unwrap();
    let stage = txn.StageStatement().unwrap();
    let version =
        super::table_mode::on_persistent_alter_table_mode(txn.as_mut(), &mut job).unwrap();
    assert_eq!(version, 1);
    assert_eq!(job.state, JobState::Done);
    let meta = astersql_meta::TransactionMutator::new(txn.as_mut());
    let table = meta.get_table(11, 22).unwrap().unwrap();
    assert_eq!(table.Mode, FullMode::TableModeImport);
    assert_eq!(table.Comment, "preserve metadata".repeat(300));
    assert_eq!(table.Revision, 1);
    txn.CleanupStatement(stage).unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let snapshot = astersql_kv::Storage::GetSnapshot(
        store.as_ref(),
        astersql_kv::Storage::CurrentVersion(store.as_ref(), "global").unwrap(),
    );
    let reader = astersql_meta::SnapshotReader::new(snapshot);
    assert_eq!(
        reader.get_table(11, 22).unwrap().unwrap().Mode,
        FullMode::TableModeNormal
    );
    let mut txn = astersql_kv::Storage::Begin(store.as_ref(), &[]).unwrap();
    let stage = txn.StageStatement().unwrap();
    let ver = super::table_mode::on_persistent_alter_table_mode(txn.as_mut(), &mut job).unwrap();
    assert_eq!(ver, 1);
    txn.ReleaseStatement(stage).unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let txn = astersql_kv::Storage::Begin(store.as_ref(), &[]).unwrap();
    let raw = txn
        .Get(
            &astersql_kv::Context::default(),
            astersql_meta::transaction_meta_string_key(b"Diff:1"),
            &[],
        )
        .unwrap()
        .Value;
    let diff: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(diff["type"], 75);
    assert_eq!(diff["schema_id"], 11);
    assert_eq!(diff["table_id"], 22);
    // An actual KV size failure after writing version/diff must not leave a
    // phantom schema version when the normal action stage is discarded.
    struct Limit(u64);
    impl Drop for Limit {
        fn drop(&mut self) {
            astersql_kv::TxnTotalSizeLimit.store(self.0, std::sync::atomic::Ordering::Release);
        }
    }
    let guard =
        Limit(astersql_kv::TxnTotalSizeLimit.swap(2000, std::sync::atomic::Ordering::AcqRel));
    job.raw_args = br#"[{"table_mode":0}]"#.to_vec();
    let mut txn = astersql_kv::Storage::Begin(store.as_ref(), &[]).unwrap();
    let stage = txn.StageStatement().unwrap();
    assert!(super::table_mode::on_persistent_alter_table_mode(txn.as_mut(), &mut job).is_err());
    txn.CleanupStatement(stage).unwrap();
    drop(guard);
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let mut check = astersql_kv::Storage::Begin(store.as_ref(), &[]).unwrap();
    assert_eq!(
        astersql_kv::GetInt64(
            &astersql_kv::Context::default(),
            check.as_ref(),
            &astersql_meta::transaction_meta_string_key(b"SchemaVersionKey")
        )
        .unwrap(),
        1
    );
    assert!(
        check
            .Get(
                &astersql_kv::Context::default(),
                astersql_meta::transaction_meta_string_key(b"Diff:2"),
                &[]
            )
            .is_err()
    );
    check.Rollback().unwrap();
}
#[test]
fn crossks_align_normal_ddl_error_wire_is_structured_and_roundtrips() {
    let mut job = astersql_meta_model::group_3::Job {
        error: Some("[schema:8259]invalid transition".into()),
        ..Default::default()
    };
    let wire = astersql_meta::encode_go_ddl_job(&mut job, false).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    assert_eq!(value["err"]["class"], 14);
    assert_eq!(value["err"]["code"], 8259);
    let decoded = astersql_meta::decode_go_history_job(&wire).unwrap();
    assert_eq!(decoded.error, job.error);
}

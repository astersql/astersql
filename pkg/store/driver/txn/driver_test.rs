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

// 事务驱动错误格式化测试：锁未找到与写冲突的可读键输出。
//
// 写冲突（write conflict）指乐观事务提交时发现键已被其他事务写入。
// 日志脱敏（redact）开启时，键详情会被掩码或用标记包裹。

use astersql_kv as kv;
use errors::{RedactLogEnable, RedactLogEnabled, RedactLogMarker};

use crate::*;

/// 从 TxnLockNotFound 错误原文中解析并美化打印嵌入的键字节数组。
#[test]
fn TestLockNotFoundPrint() {
    let msg = "Txn(Mvcc(TxnLockNotFound { start_ts: 408090278408224772, commit_ts: 408090279311835140, \
        key: [116, 128, 0, 0, 0, 0, 0, 50, 137, 95, 105, 128, 0, 0, 0, 0,0 ,0, 1, 1, 67, 49, 57, 48, 57, 50, 57, 48, 255, 48, 48, 48, 48, 48, 52, 56, 54, 255, 50, 53, 53, 50, 51, 0, 0, 0, 252] }))";
    let key = prettyLockNotFoundKey(msg);
    let expected = "{tableID=12937, indexID=1, indexValues={C19092900000048625523, }}";
    assert_eq!(key, expected);
}

/// 验证写冲突错误在普通键、meta 键及不同脱敏模式下的格式化输出。
#[test]
fn TestWriteConflictPrettyFormat() {
    let mut conflict = WriteConflict {
        start_ts: 399402937522847774,
        conflict_ts: 399402937719455772,
        conflict_commit_ts: 399402937719455773,
        key: vec![
            116, 128, 0, 0, 0, 0, 0, 1, 155, 95, 105, 128, 0, 0, 0, 0, 0, 0, 1, 1, 82, 87, 48, 49,
            0, 0, 0, 0, 251, 1, 55, 54, 56, 50, 50, 49, 49, 48, 255, 57, 0, 0, 0, 0, 0, 0, 0, 248,
            1, 0, 0, 0, 0, 0, 0, 0, 0, 247,
        ],
        primary: vec![
            116, 128, 0, 0, 0, 0, 0, 1, 155, 95, 105, 128, 0, 0, 0, 0, 0, 0, 1, 1, 82, 87, 48, 49,
            0, 0, 0, 0, 251, 1, 55, 54, 56, 50, 50, 49, 49, 48, 255, 57, 0, 0, 0, 0, 0, 0, 0, 248,
            1, 0, 0, 0, 0, 0, 0, 0, 0, 247,
        ],
        reason: "Unknown".to_owned(),
        ..Default::default()
    };

    // 普通索引键：美化 tableID / indexValues，并附带十六进制 originalKey。
    let mut expected = format!(
        "[kv:9007]Write conflict, txnStartTS=399402937522847774, conflictStartTS=399402937719455772, conflictCommitTS=399402937719455773, \
key={{tableID=411, indexID=1, indexValues={{RW01, 768221109, , }}}}, \
originalKey=74800000000000019b5f698000000000000001015257303100000000fb013736383232313130ff3900000000000000f8010000000000000000f7, \
primary={{tableID=411, indexID=1, indexValues={{RW01, 768221109, , }}}}, \
originalPrimaryKey=74800000000000019b5f698000000000000001015257303100000000fb013736383232313130ff3900000000000000f8010000000000000000f7, \
reason=Unknown {}",
        kv::TxnRetryableMark
    );
    assert_eq!(
        newWriteConflictError(Some(conflict.clone())).to_string(),
        expected
    );

    // Meta 键（以 m 开头）走 metaKey 格式化分支。
    conflict.key = vec![
        0x6d, 0x44, 0x42, 0x3a, 0x35, 0x36, 0x0, 0x0, 0x0, 0xfc, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
        0x68, 0x54, 0x49, 0x44, 0x3a, 0x31, 0x30, 0x38, 0x0, 0xfe,
    ];
    conflict.primary = conflict.key.clone();
    conflict.reason = "Optimistic".to_owned();
    expected = format!(
        "[kv:9007]Write conflict, txnStartTS=399402937522847774, conflictStartTS=399402937719455772, conflictCommitTS=399402937719455773, \
key={{metaKey=true, key=DB:56, field=TID:108}}, \
originalKey=6d44423a3536000000fc00000000000000685449443a31303800fe, \
primary={{metaKey=true, key=DB:56, field=TID:108}}, \
originalPrimaryKey=6d44423a3536000000fc00000000000000685449443a31303800fe, \
reason=Optimistic {}",
        kv::TxnRetryableMark
    );
    assert_eq!(
        newWriteConflictError(Some(conflict.clone())).to_string(),
        expected
    );

    // 全脱敏：键内容替换为 ????。
    let original = RedactLogEnabled.Load();
    RedactLogEnabled.Store(RedactLogEnable);
    expected = format!(
        "[kv:9007]Write conflict, txnStartTS=399402937522847774, conflictStartTS=399402937719455772, conflictCommitTS=399402937719455773, \
key=????, reason=Optimistic {}",
        kv::TxnRetryableMark
    );
    assert_eq!(
        newWriteConflictError(Some(conflict.clone())).to_string(),
        expected
    );

    // 标记脱敏：敏感片段用 ‹› 包裹。
    RedactLogEnabled.Store(RedactLogMarker);
    expected = format!(
        "[kv:9007]Write conflict, txnStartTS=399402937522847774, conflictStartTS=399402937719455772, conflictCommitTS=399402937719455773, \
key=‹›‹{{metaKey=true, key=DB:56, field=TID:108}}, originalKey=6d44423a3536000000fc00000000000000685449443a31303800fe, primary=›‹›‹{{metaKey=true, key=DB:56, field=TID:108}}, originalPrimaryKey=6d44423a3536000000fc00000000000000685449443a31303800fe›, reason=Optimistic {}",
        kv::TxnRetryableMark
    );
    assert_eq!(newWriteConflictError(Some(conflict)).to_string(), expected);
    RedactLogEnabled.Store(original);
}

/// 官方事务模式和错误/未命中值必须映射到 canonical KV 契约。
#[test]
fn client_rust_mode_and_error_mapping_match_canonical_contract() {
    assert_eq!(ClientTransactionMode::Optimistic.as_str(), "optimistic");
    assert_eq!(ClientTransactionMode::Pessimistic.as_str(), "pessimistic");
    assert!(format!("{:?}", ClientTransactionMode::Optimistic.options()).contains("Optimistic"));
    assert!(format!("{:?}", ClientTransactionMode::Pessimistic.options()).contains("Pessimistic"));

    let error = map_client_error(tikv_client::Error::StringError(
        "client-rust failure".to_owned(),
    ));
    assert!(error.to_string().contains("client-rust failure"));
    assert_eq!(
        canonical_value(Some(b"value".to_vec()))
            .expect("present value must map")
            .Value,
        b"value"
    );
    assert!(canonical_value(None).is_err());
}

#[test]
fn generic_client_error_mapping_avoids_synchronous_stack_symbolization() {
    let error = map_client_error(tikv_client::Error::ResolveLockError(vec![
        tikv_client::proto::kvrpcpb::LockInfo::default(),
    ]));
    assert_eq!(error.to_string(), "Failed to resolve lock");
    assert!(!kv::errors::HasStack(&error));
}

/// 重复键错误提取必须直接接受 tablecodec 暴露的 canonical model.TableInfo。
#[test]
fn canonical_table_info_drives_duplicate_handle_error() {
    let mut table = astersql_tablecodec::model::TableInfo::default();
    table.ID = 42;
    table.Name.O = "accounts".to_owned();
    table.Name.L = "accounts".to_owned();

    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        table.ID,
        Box::new(astersql_tablecodec::kv::IntHandle(-7)),
    );
    let error = ExtractKeyExistsErrFromHandle(&key.0, &[], &table);

    assert_eq!(
        error.to_string(),
        "Duplicate entry '-7' for key 'accounts.PRIMARY'"
    );

    let mut primary_column = astersql_tablecodec::model::ColumnInfo::default();
    primary_column.ID = 1;
    primary_column.Offset = 0;
    primary_column
        .FieldType
        .SetFlag(astersql_tablecodec::mysql::PriKeyFlag | astersql_tablecodec::mysql::UnsignedFlag);
    table.Columns.push(primary_column);
    let error = ExtractKeyExistsErrFromHandle(&key.0, &[], &table);
    assert_eq!(
        error.to_string(),
        format!(
            "Duplicate entry '{}' for key 'accounts.PRIMARY'",
            (-7_i64) as u64
        )
    );
}

/// 唯一索引重复键值必须经 tablecodec 的真实 Datum 编解码路径还原。
#[test]
fn canonical_index_metadata_decodes_duplicate_value() {
    let mut column = astersql_tablecodec::model::ColumnInfo::default();
    column.ID = 1;
    column.Offset = 0;
    column
        .FieldType
        .SetType(astersql_tablecodec::mysql::TypeLonglong);

    let mut index_column = astersql_tablecodec::model::IndexColumn::default();
    index_column.Offset = 0;
    index_column.Length = astersql_tablecodec::types::UnspecifiedLength as isize;

    let mut index = astersql_tablecodec::model::IndexInfo::default();
    index.ID = 9;
    index.Name.O = "uniq_account".to_owned();
    index.Name.L = "uniq_account".to_owned();
    index.Unique = true;
    index.Columns.push(index_column);

    let mut table = astersql_tablecodec::model::TableInfo::default();
    table.ID = 42;
    table.Name.O = "accounts".to_owned();
    table.Name.L = "accounts".to_owned();
    table.Columns.push(column);
    table.Indices.push(index);

    let encoded = astersql_tablecodec::codec::EncodeKey(
        astersql_tablecodec::time::UTC,
        Vec::new(),
        vec![astersql_tablecodec::types::NewIntDatum(123)],
    )
    .unwrap();
    let key = astersql_tablecodec::EncodeIndexSeekKey(table.ID, 9, Some(encoded));
    let value = astersql_tablecodec::codec::EncodeInt(Vec::new(), 1);
    let error = ExtractKeyExistsErrFromIndex(&key.0, &value, &table, 9);

    assert_eq!(
        error.to_string(),
        "Duplicate entry '123' for key 'accounts.uniq_account'"
    );
}

#[test]
fn client_prewrite_conflict_retains_canonical_error_code_and_reason() {
    let error = tikv_client::Error::MultipleKeyErrors(vec![tikv_client::Error::KeyError(
        Box::new(tikv_client::proto::kvrpcpb::KeyError {
            conflict: Some(tikv_client::proto::kvrpcpb::WriteConflict {
                start_ts: 10,
                conflict_ts: 11,
                conflict_commit_ts: 12,
                reason: tikv_client::proto::kvrpcpb::write_conflict::Reason::SelfRolledBack as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
    )]);
    let mapped = crate::map_client_error(error);
    assert!(astersql_kv::ErrWriteConflict.Equal(Some(&mapped)));
    assert!(mapped.to_string().contains("SelfRolledBack"));
}

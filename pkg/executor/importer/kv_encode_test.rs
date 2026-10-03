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

use std::sync::Arc;

use astersql_kv::Key;
use astersql_lightning_backend_encode::{Datum, EncodingConfig, SessionOptions};
use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, TableInfo, ast};
use astersql_parser_mysql::r#type::{TypeEnum, TypeLonglong, TypeTiny};
use astersql_tablecodec::{DecodeRowKey, IsRecordKey};
use astersql_types::StrictContext;

use crate::{CanonicalImportDatumConverter, NewTableDefinitionFromMeta, NewTableKVEncoderFromMeta};

fn column(id: i64, name: &str, field_type: u8) -> ColumnInfo {
    let mut column = ColumnInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        Offset: 0,
        State: StatePublic,
        ..ColumnInfo::default()
    };
    column.SetType(field_type);
    column
}

fn encoder_for(meta: &TableInfo, identity: bool) -> crate::TableKVEncoder {
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(meta).unwrap())),
        UseIdentityAutoRowID: identity,
        SessionOptions: SessionOptions {
            AutoRandomSeed: 0,
            ..SessionOptions::default()
        },
        ..EncodingConfig::default()
    };
    NewTableKVEncoderFromMeta(
        &config,
        meta,
        Arc::new(CanonicalImportDatumConverter(StrictContext.Flags())),
    )
    .unwrap()
}

#[test]
fn integer_primary_handle_uses_column_flag_without_primary_index() {
    let mut primary = column(1, "id", TypeLonglong);
    primary.SetFlag(astersql_parser_mysql::r#type::PriKeyFlag);
    let meta = TableInfo {
        ID: 42,
        Name: ast::NewCIStr("t"),
        Columns: vec![primary],
        PKIsHandle: true,
        ..TableInfo::default()
    };
    let mut encoder = encoder_for(&meta, false);
    let pairs = encoder.Encode(&[Datum::Int(123)], 999).unwrap();
    assert_eq!(pairs.Pairs.len(), 1);
    let record = &pairs.Pairs[0];
    assert!(IsRecordKey(&record.key));
    assert_eq!(
        DecodeRowKey(Key(record.key.clone())).unwrap().IntValue(),
        123
    );
    encoder.Close().unwrap();
}

#[test]
fn duplicate_resolution_respects_identity_and_sharded_row_ids() {
    let primary = IndexInfo {
        ID: 11,
        Name: ast::NewCIStr("PRIMARY"),
        State: StatePublic,
        Primary: true,
        Unique: true,
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr("a"),
            Offset: 0,
            ..IndexColumn::default()
        }],
        ..IndexInfo::default()
    };
    let meta = TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        Columns: vec![column(1, "a", TypeLonglong)],
        Indices: vec![primary],
        ShardRowIDBits: 6,
        ..TableInfo::default()
    };

    for identity in [true, false] {
        let mut encoder = encoder_for(&meta, identity);
        let mut sharded = 0;
        for _ in 0..10 {
            let pairs = encoder.Encode(&[Datum::Int(1)], 1).unwrap();
            assert_eq!(pairs.Pairs.len(), 2);
            let record = pairs
                .Pairs
                .iter()
                .find(|pair| IsRecordKey(&pair.key))
                .unwrap();
            let handle = DecodeRowKey(Key(record.key.clone())).unwrap().IntValue();
            if identity {
                assert_eq!(handle, 1);
            } else if handle > 1 {
                sharded += 1;
            }
        }
        if !identity {
            assert!(sharded > 1);
        }
        encoder.Close().unwrap();
    }
}

#[test]
fn tinyint_cast_error_preserves_import_context() {
    let meta = TableInfo {
        ID: 2,
        Name: ast::NewCIStr("t"),
        Columns: vec![column(1, "c1", TypeTiny)],
        ..TableInfo::default()
    };
    let mut encoder = encoder_for(&meta, true);
    let error = encoder.Encode(&[Datum::Int(10_000_000)], 1).unwrap_err();
    assert!(
        error.contains("Value conversion failed for column 'c1'"),
        "{error}"
    );
    assert!(error.contains("10000000"), "{error}");
    assert!(error.contains("tinyint"), "{error}");
    encoder.Close().unwrap();
}

#[test]
fn enum_cast_error_preserves_value_and_truncation_reason() {
    let mut enum_column = column(1, "c1", TypeEnum);
    enum_column.SetElems(vec!["a".into(), "b".into()]);
    let meta = TableInfo {
        ID: 3,
        Name: ast::NewCIStr("t"),
        Columns: vec![enum_column],
        ..TableInfo::default()
    };
    let mut encoder = encoder_for(&meta, true);
    let error = encoder.Encode(&[Datum::String("c".into())], 1).unwrap_err();
    assert!(
        error.contains("Value conversion failed for column 'c1'"),
        "{error}"
    );
    assert!(error.contains("enum('a','b')"), "{error}");
    assert!(error.contains("\"c\""), "{error}");
    assert!(error.contains("Data truncated"), "{error}");
    encoder.Close().unwrap();
}

#[test]
fn persistent_varchar_metadata_preserves_restored_index_values_and_primary_handle() {
    let mut primary = column(7, "a", TypeLonglong);
    primary.SetFlag(astersql_parser_mysql::r#type::PriKeyFlag);
    let mut text = column(12, "b", astersql_parser_mysql::r#type::TypeVarchar);
    text.Offset = 1;
    text.SetCharset("utf8mb4".into());
    text.SetCollate("utf8mb4_bin".into());
    text.SetFlen(100);
    let meta = TableInfo {
        ID: 42,
        Name: ast::NewCIStr("t"),
        Columns: vec![primary, text],
        Indices: vec![IndexInfo {
            ID: 1,
            State: StatePublic,
            Columns: vec![IndexColumn {
                Name: ast::NewCIStr("b"),
                Offset: 1,
                Length: -1,
                ..Default::default()
            }],
            ..Default::default()
        }],
        PKIsHandle: true,
        ..Default::default()
    };
    let mut encoder = encoder_for(&meta, true);
    let pairs = encoder
        .Encode(&[Datum::Int(123), Datum::String("test-123 ".into())], 999)
        .unwrap();
    assert_eq!(pairs.Pairs.len(), 2);
    let record = pairs.Pairs.iter().find(|p| IsRecordKey(&p.key)).unwrap();
    let columns = meta
        .Columns
        .iter()
        .map(|c| (c.ID, Box::new(c.FieldType.clone())))
        .collect();
    let decoded = astersql_tablecodec::DecodeRowToDatumMap(
        Some(record.val.clone()),
        columns,
        Some(astersql_tablecodec::time::UTC),
    )
    .unwrap();
    assert!(
        !decoded.contains_key(&7),
        "integer primary key belongs to the handle"
    );
    assert_eq!(decoded[&12].GetString(), "test-123 ");
    assert_eq!(
        DecodeRowKey(Key(record.key.clone())).unwrap().IntValue(),
        123
    );
    let index = pairs.Pairs.iter().find(|p| !IsRecordKey(&p.key)).unwrap();
    assert!(
        index.val.len() > 1,
        "VARCHAR index must retain restored values under new collation"
    );
    let restored = astersql_tablecodec::DecodeRowToDatumMap(
        Some(index.val[1..].to_vec()),
        [(12, Box::new(meta.Columns[1].FieldType.clone()))]
            .into_iter()
            .collect(),
        Some(astersql_tablecodec::time::UTC),
    )
    .unwrap();
    assert_eq!(restored[&12].GetString(), "test-123 ");
    encoder.Close().unwrap();
}

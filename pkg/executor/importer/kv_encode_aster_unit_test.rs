// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// KV 编码与分组批处理的 Aster 单元测试。
//
// 验证 BaseKVEncoder 输出拆分为记录/索引 KV、checksum 累加，
// 以及索引元信息过滤与非法 key 时的字节回退语义。

use std::sync::Arc;

use astersql_lightning_backend_encode::{Column, Datum, EncodingConfig};
use astersql_lightning_backend_kv::{IndexDefinition, NewBaseKVEncoder, Pairs, TableDefinition};
use astersql_lightning_verification::KvPair;
use astersql_meta_model::{IndexInfo, SchemaState, StatePublic, TableInfo, ast};

use crate::*;

#[test]
fn metadata_import_encoder_casts_text_and_preserves_hidden_row_handle() {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column.ID = 1;
    column.Name = ast::NewCIStr("value");
    column.SetType(astersql_parser_mysql::r#type::TypeLonglong);
    let meta = TableInfo {
        ID: 42,
        Name: ast::NewCIStr("t"),
        Columns: vec![column],
        ..Default::default()
    };
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&meta).unwrap())),
        ..Default::default()
    };
    let mut encoder = NewTableKVEncoderFromMeta(
        &config,
        &meta,
        Arc::new(CanonicalImportDatumConverter(
            astersql_types::StrictContext.Flags(),
        )),
    )
    .unwrap();
    let row = encoder.Encode(&[Datum::String("7".into())], 321).unwrap();
    assert_eq!(row.Pairs.len(), 1);
    assert!(row.Pairs[0].key.ends_with(&[0x80, 0, 0, 0, 0, 0, 1, 0x41]));
    assert!(
        encoder
            .Encode(&[Datum::String("not-an-integer".into())], 322)
            .is_err()
    );
    encoder.Close().unwrap();
}

/// 构造含主键列与二级索引的简易 EncodingConfig。
fn encoding_config() -> EncodingConfig {
    let table = TableDefinition {
        id: 42,
        name: "t".to_owned(),
        columns: vec![
            Column {
                name: "id".to_owned(),
                primary_key: true,
                ..Column::default()
            },
            Column {
                name: "value".to_owned(),
                ..Column::default()
            },
        ],
        indices: vec![IndexDefinition {
            id: 9,
            columns: vec![1],
            primary: false,
            unique: false,
        }],
        pk_is_handle: true,
        ..TableDefinition::default()
    };
    EncodingConfig {
        Table: Some(Arc::new(table)),
        ..EncodingConfig::default()
    }
}

/// 验证 Record2KV 产出 data+index 两对，且 EncodedKVGroupBatch 正确分组与 checksum。
#[test]
fn base_encoder_output_is_split_and_checksummed_by_record_and_index() {
    let mut encoder = NewBaseKVEncoder(&encoding_config()).unwrap();
    let row = vec![Datum::Int(7), Datum::String("v".to_owned())];
    let pairs = encoder.Record2KV(row.clone(), &row, 100).unwrap();
    assert_eq!(pairs.Pairs.len(), 2);

    // 原始字节数 = 所有 key+val 长度之和；分组后 checksum 另含 keyspace 开销。
    let raw_bytes: i64 = pairs
        .Pairs
        .iter()
        .map(|pair| (pair.key.len() + pair.val.len()) as i64)
        .sum();
    let mut batch = NewEncodedKVGroupBatch(b"ks", 1);
    assert_eq!(batch.Add(&pairs).unwrap(), raw_bytes);
    assert_eq!(batch.data_kvs.len(), 1);
    assert_eq!(batch.index_kvs[&9].len(), 1);
    assert_eq!(batch.group_checksum.DataAndIndexSumKVS(), (1, 1));
    let (data_size, index_size) = batch.group_checksum.DataAndIndexSumSize();
    assert_eq!(data_size + index_size, raw_bytes as u64 + 4);
    encoder.SessionCtx.Close();
}

/// 构造测试用 IndexInfo。
fn index(id: i64, name: &str, state: SchemaState, primary: bool, unique: bool) -> IndexInfo {
    IndexInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        State: state,
        Primary: primary,
        Unique: unique,
        ..IndexInfo::default()
    }
}

/// 验证 GetIndicesGenKV 只保留 Public 且非聚簇主键的二级索引。
#[test]
fn metadata_filter_keeps_public_non_clustered_indices() {
    let table = TableInfo {
        PKIsHandle: true,
        Indices: vec![
            index(1, "PRIMARY", StatePublic, true, true),
            index(2, "unique_value", StatePublic, false, true),
            index(3, "building", SchemaState::WriteOnly, false, false),
        ],
        ..TableInfo::default()
    };
    let generated = GetIndicesGenKV(&table);
    assert_eq!(generated.len(), 1);
    assert_eq!(generated[&2].name, "unique_value");
    assert!(generated[&2].Unique);
    assert_eq!(GetNumOfIndexGenKV(&table), 1);
}

/// 验证遇到非法 key 时 Add 返回已累计字节，且已写入的 data kv 保留。
#[test]
fn grouping_returns_bytes_before_an_invalid_key() {
    let valid = KvPair {
        key: b"t1_r1".to_vec(),
        val: b"value".to_vec(),
    };
    let invalid = KvPair {
        key: b"not-a-table-key".to_vec(),
        val: Vec::new(),
    };
    let expected_bytes = (valid.key.len() + valid.val.len()) as i64;
    let pairs = Pairs {
        Pairs: vec![valid, invalid],
        ..Pairs::default()
    };
    let mut batch = NewEncodedKVGroupBatch(&[], 1);
    let (bytes, error) = batch.Add(&pairs).unwrap_err();
    assert_eq!(bytes, expected_bytes);
    assert_eq!(batch.data_kvs.len(), 1);
    assert!(error.contains("neither a record key nor an index key"));
}

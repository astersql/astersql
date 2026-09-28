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

// keydecoder 单测：行键/索引键解码、分区身份、缺失 schema 与诊断 JSON 字段约定。

use crate::keydecoder::{
    CommonHandle, DecodedKey, IntHandle, KeyMetadata, MetadataLookup, decodeKeyWithLookup,
};
use astersql_infoschema as infoschema;
use astersql_tablecodec as tablecodec;
use astersql_tablecodec::kv;
use astersql_types as types;
use astersql_util_codec as codec;

/// 固定返回预设 `KeyMetadata` 的测试用查找器。
#[derive(Default)]
struct Lookup {
    metadata: KeyMetadata,
}

impl MetadataLookup for Lookup {
    fn lookup(&self, _table_or_partition_id: i64) -> KeyMetadata {
        self.metadata.clone()
    }
}

/// 构造带库/表/索引元数据的查找器，模拟 infoschema 命中普通表。
fn table_lookup(table_id: i64) -> Lookup {
    Lookup {
        metadata: KeyMetadata {
            db_id: 10,
            db_name: "test".into(),
            table_id,
            table_name: "t".into(),
            indices: vec![(7, "idx".into())],
            table_found: true,
            ..KeyMetadata::default()
        },
    }
}

fn ci(value: &str) -> infoschema::CiString {
    infoschema::CiString::new(value)
}

/// 非 TiDB 行/索引键应报 Unknown key type。
#[test]
fn rejects_unknown_key_types() {
    let error = decodeKeyWithLookup(b"not-a-tidb-key", &Lookup::default()).unwrap_err();
    assert!(error.to_string().contains("Unknown key type"));
}

/// 整数 Handle 行键：填充表/库身份与 Handle 类型、值。
#[test]
fn decodes_integer_record_key_and_schema_identity() {
    let key = tablecodec::EncodeRowKeyWithHandle(42, Box::new(kv::IntHandle(123)));
    let decoded = decodeKeyWithLookup(key.as_ref(), &table_lookup(42)).unwrap();
    assert_eq!(decoded.TableID, 42);
    assert_eq!(decoded.TableName, "t");
    assert_eq!(decoded.DbID, 10);
    assert_eq!(decoded.DbName, "test");
    assert_eq!(decoded.HandleType, IntHandle);
    assert_eq!(decoded.HandleValue, "123");
    assert!(!decoded.IsPartitionHandle);
}

/// 公共 Handle（多列主键编码）行键的类型与字符串表示。
#[test]
fn decodes_common_record_handle() {
    let encoded = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![
            types::datum::NewIntDatum(7),
            types::datum::NewStringDatum("abc".into()),
        ],
    )
    .unwrap();
    let common = kv::NewCommonHandle(encoded).unwrap();
    let key = tablecodec::EncodeRowKeyWithHandle(42, Box::new(common));
    let decoded = decodeKeyWithLookup(key.as_ref(), &table_lookup(42)).unwrap();
    assert_eq!(decoded.HandleType, CommonHandle);
    assert_eq!(decoded.HandleValue, "{7, abc}");
}

/// 分区物理 ID 解码后应保留逻辑表 ID 与分区名。
#[test]
fn preserves_partition_identity() {
    let key = tablecodec::EncodeRowKeyWithHandle(9, Box::new(kv::IntHandle(3)));
    let lookup = Lookup {
        metadata: KeyMetadata {
            db_id: 1,
            db_name: "test".into(),
            table_id: 4,
            table_name: "pt".into(),
            partition_id: 9,
            partition_name: "p0".into(),
            table_found: true,
            ..KeyMetadata::default()
        },
    };
    let decoded = decodeKeyWithLookup(key.as_ref(), &lookup).unwrap();
    assert_eq!((decoded.TableID, decoded.PartitionID), (4, 9));
    assert_eq!(
        (decoded.TableName.as_str(), decoded.PartitionName.as_str()),
        ("pt", "p0")
    );
}

/// 索引键：解析索引 ID、名称与列值。
#[test]
fn decodes_index_name_and_values() {
    let encoded = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![
            types::datum::NewIntDatum(11),
            types::datum::NewStringDatum("value".into()),
        ],
    )
    .unwrap();
    let key = tablecodec::EncodeIndexSeekKey(42, 7, Some(encoded));
    let decoded = decodeKeyWithLookup(key.as_ref(), &table_lookup(42)).unwrap();
    assert_eq!(decoded.IndexID, 7);
    assert_eq!(decoded.IndexName, "idx");
    assert_eq!(decoded.IndexValues, vec!["11", "value"]);
}

/// 元数据未命中时仍保留物理表 ID，并继续解码 Handle。
#[test]
fn missing_table_keeps_physical_id_and_still_decodes_handle() {
    let key = tablecodec::EncodeRowKeyWithHandle(404, Box::new(kv::IntHandle(8)));
    let decoded = decodeKeyWithLookup(key.as_ref(), &Lookup::default()).unwrap();
    assert_eq!(decoded.TableID, 404);
    assert!(decoded.TableName.is_empty());
    assert_eq!(decoded.HandleValue, "8");
}

/// schema 缺失时返回部分表身份，且不继续解码 Handle。
#[test]
fn missing_schema_returns_partial_table_identity_before_handle_decode() {
    let key = tablecodec::EncodeRowKeyWithHandle(42, Box::new(kv::IntHandle(8)));
    let lookup = Lookup {
        metadata: KeyMetadata {
            table_id: 42,
            table_name: "orphan".into(),
            table_found: true,
            schema_missing_for_table: true,
            ..KeyMetadata::default()
        },
    };
    let decoded = decodeKeyWithLookup(key.as_ref(), &lookup).unwrap();
    assert_eq!(decoded.TableName, "orphan");
    assert!(decoded.HandleValue.is_empty());
}

/// 诊断 JSON 使用 Go 字段名，并省略空/零值字段。
#[test]
fn diagnostic_json_uses_go_field_names_and_omits_empty_values() {
    let decoded = DecodedKey {
        TableID: 42,
        HandleType: IntHandle,
        HandleValue: "9".into(),
        ..DecodedKey::default()
    };
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::json!({"table_id": 42, "handle_type": "int", "handle_value": "9"})
    );
}

/// 对应 Go `TestDecodeKey`：通过真实 InfoSchema 适配器覆盖普通表、公共句柄、
/// 索引键、分区键、非法键和 schema 变化后的未知表行为。
#[test]
fn decode_key_matches_go_test_decode_key_scenarios() {
    let schema = infoschema::MockInfoSchema(vec![
        infoschema::TableInfo {
            id: 1,
            name: ci("table1"),
            indices: vec![infoschema::IndexInfo {
                id: 1,
                name: ci("index1"),
            }],
            ..Default::default()
        },
        infoschema::TableInfo {
            id: 2,
            name: ci("table2"),
            ..Default::default()
        },
        infoschema::TableInfo {
            id: 3,
            name: ci("table3"),
            indices: vec![infoschema::IndexInfo {
                id: 4,
                name: ci("index4"),
            }],
            partition: Some(infoschema::PartitionInfo {
                definitions: vec![
                    infoschema::PartitionDefinition {
                        id: 5,
                        name: ci("p0"),
                    },
                    infoschema::PartitionDefinition {
                        id: 6,
                        name: ci("p1"),
                    },
                ],
            }),
            ..Default::default()
        },
    ]);

    let key = tablecodec::EncodeRowKeyWithHandle(1, Box::new(kv::IntHandle(1)));
    let decoded = crate::DecodeKey(key.as_ref(), schema.as_ref()).unwrap();
    assert_eq!(decoded.DbID, 1);
    assert_eq!(decoded.DbName, "test");
    assert_eq!(decoded.TableID, 1);
    assert_eq!(decoded.TableName, "table1");
    assert_eq!(decoded.HandleType, IntHandle);
    assert_eq!(decoded.HandleValue, "1");
    assert_eq!(decoded.PartitionID, 0);
    assert!(decoded.PartitionName.is_empty());
    assert!(!decoded.IsPartitionHandle);
    assert_eq!(decoded.IndexID, 0);
    assert!(decoded.IndexName.is_empty());
    assert!(decoded.IndexValues.is_empty());

    let encoded_common = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![
            types::datum::NewIntDatum(100),
            types::datum::NewStringDatum("abc".into()),
        ],
    )
    .unwrap();
    let common = kv::NewCommonHandle(encoded_common).unwrap();
    let key = tablecodec::EncodeRowKeyWithHandle(2, Box::new(common));
    let decoded = crate::DecodeKey(key.as_ref(), schema.as_ref()).unwrap();
    assert_eq!(decoded.DbID, 1);
    assert_eq!(decoded.DbName, "test");
    assert_eq!(decoded.TableID, 2);
    assert_eq!(decoded.TableName, "table2");
    assert_eq!(decoded.HandleType, CommonHandle);
    assert_eq!(decoded.HandleValue, "{100, abc}");
    assert!(decoded.IndexValues.is_empty());

    let encoded_values = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![
            types::datum::NewStringDatum("abc".into()),
            types::datum::NewIntDatum(1),
        ],
    )
    .unwrap();
    let key = tablecodec::EncodeIndexSeekKey(1, 1, Some(encoded_values));
    let decoded = crate::DecodeKey(key.as_ref(), schema.as_ref()).unwrap();
    assert_eq!(decoded.DbID, 1);
    assert_eq!(decoded.DbName, "test");
    assert_eq!(decoded.TableID, 1);
    assert_eq!(decoded.TableName, "table1");
    assert_eq!(decoded.IndexID, 1);
    assert_eq!(decoded.IndexName, "index1");
    assert_eq!(decoded.IndexValues, vec!["abc", "1"]);
    assert!(decoded.HandleType.is_empty());
    assert!(decoded.HandleValue.is_empty());
    assert!(!decoded.IsPartitionHandle);

    let key = tablecodec::EncodeRowKeyWithHandle(5, Box::new(kv::IntHandle(10)));
    let decoded = crate::DecodeKey(key.as_ref(), schema.as_ref()).unwrap();
    assert_eq!(decoded.DbID, 1);
    assert_eq!(decoded.DbName, "test");
    assert_eq!(decoded.TableID, 3);
    assert_eq!(decoded.TableName, "table3");
    assert_eq!(decoded.PartitionID, 5);
    assert_eq!(decoded.PartitionName, "p0");
    assert_eq!(decoded.HandleType, IntHandle);
    assert_eq!(decoded.HandleValue, "10");
    assert!(decoded.IndexValues.is_empty());

    let encoded_values = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![
            types::datum::NewStringDatum("abcde".into()),
            types::datum::NewIntDatum(2),
        ],
    )
    .unwrap();
    let key = tablecodec::EncodeIndexSeekKey(6, 4, Some(encoded_values));
    let decoded = crate::DecodeKey(key.as_ref(), schema.as_ref()).unwrap();
    assert_eq!(decoded.DbID, 1);
    assert_eq!(decoded.DbName, "test");
    assert_eq!(decoded.TableID, 3);
    assert_eq!(decoded.TableName, "table3");
    assert_eq!(decoded.PartitionID, 6);
    assert_eq!(decoded.PartitionName, "p1");
    assert_eq!(decoded.IndexID, 4);
    assert_eq!(decoded.IndexName, "index4");
    assert_eq!(decoded.IndexValues, vec!["abcde", "2"]);
    assert!(decoded.HandleType.is_empty());
    assert!(decoded.HandleValue.is_empty());
    assert!(!decoded.IsPartitionHandle);

    assert!(crate::DecodeKey(b"this-is-a-totally-invalidkey", schema.as_ref()).is_err());

    let mut partly_invalid = tablecodec::GenTableRecordPrefix(1).0;
    partly_invalid.extend_from_slice(b"rest-part-is-invalid");
    assert!(crate::DecodeKey(&partly_invalid, schema.as_ref()).is_err());

    let key = tablecodec::EncodeRowKeyWithHandle(4, Box::new(kv::IntHandle(1)));
    let decoded = crate::DecodeKey(key.as_ref(), schema.as_ref()).unwrap();
    assert_eq!(decoded.TableID, 4);
    assert!(decoded.TableName.is_empty());
    assert_eq!(decoded.HandleType, IntHandle);
    assert_eq!(decoded.HandleValue, "1");
    assert_eq!(decoded.DbID, 0);
    assert!(decoded.DbName.is_empty());
    assert_eq!(decoded.PartitionID, 0);
    assert!(decoded.PartitionName.is_empty());
    assert_eq!(decoded.IndexID, 0);
    assert!(decoded.IndexName.is_empty());
    assert!(decoded.IndexValues.is_empty());
    assert!(!decoded.IsPartitionHandle);
}

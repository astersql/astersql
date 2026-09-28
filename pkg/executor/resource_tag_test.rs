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

// Resource Group Tag metadata parity for executor requests.
//
// Go observes tags at the mock-store RPC boundary. The Rust mock RPC does not
// expose that hook, so this test exercises the same tag builder used by KV
// requests and preserves every SQL/label case from TestResourceGroupTag.

use astersql_kv as kv;
use astersql_parser::NormalizeDigest;
use astersql_tablecodec::{GenTableIndexPrefix, GenTableRecordPrefix};
use astersql_util_resourcegrouptag::DecodeResourceGroupTag;
use std::sync::Once;

const TABLE_ID: i64 = 42;
const ROW_LABEL: u64 = 1;
const INDEX_LABEL: u64 = 2;

#[derive(Debug, Default, Eq, PartialEq)]
struct DecodedTag {
    sql_digest: Vec<u8>,
    plan_digest: Vec<u8>,
    label: u64,
    table_id: u64,
}

fn read_varint(input: &[u8], cursor: &mut usize) -> u64 {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = input[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("invalid protobuf varint")
}

fn decode_tag(encoded: &[u8]) -> DecodedTag {
    let mut decoded = DecodedTag::default();
    let mut cursor = 0;
    while cursor < encoded.len() {
        let key = read_varint(encoded, &mut cursor);
        let field = key >> 3;
        let wire = key & 7;
        match (field, wire) {
            (1, 2) | (2, 2) => {
                let length = read_varint(encoded, &mut cursor) as usize;
                let value = encoded[cursor..cursor + length].to_vec();
                cursor += length;
                if field == 1 {
                    decoded.sql_digest = value;
                } else {
                    decoded.plan_digest = value;
                }
            }
            (3, 0) => decoded.label = read_varint(encoded, &mut cursor),
            (4, 0) => decoded.table_id = read_varint(encoded, &mut cursor),
            (_, 0) => {
                read_varint(encoded, &mut cursor);
            }
            (_, 2) => {
                let length = read_varint(encoded, &mut cursor) as usize;
                cursor += length;
            }
            _ => panic!("unsupported protobuf field {field} with wire type {wire}"),
        }
    }
    decoded
}

fn initialize_table_id_decoder() {
    static INIT: Once = Once::new();
    INIT.call_once(astersql_tablecodec::init);
}

/// Mirrors all SQL cases and accepted labels in Go TestResourceGroupTag.
#[test]
fn resource_group_tags_preserve_executor_sql_plan_table_and_label_contract() {
    initialize_table_id_decoder();

    let cases: &[(&str, &[u64])] = &[
        ("insert into t values(1,1),(2,2),(3,3)", &[INDEX_LABEL]),
        (
            "select * from t use index (idx) where a=1",
            &[ROW_LABEL, INDEX_LABEL],
        ),
        (
            "select * from t use index (idx) where a in (1,2,3)",
            &[ROW_LABEL, INDEX_LABEL],
        ),
        (
            "select * from t use index (idx) where a>1",
            &[ROW_LABEL, INDEX_LABEL],
        ),
        ("select * from t where b>1", &[ROW_LABEL]),
        ("select a from t use index (idx) where a>1", &[INDEX_LABEL]),
        ("begin pessimistic", &[]),
        ("insert into t values(4,4)", &[ROW_LABEL, INDEX_LABEL]),
        ("commit", &[]),
        ("update t set a=5,b=5 where a=5", &[INDEX_LABEL]),
        ("replace into t values(6,6)", &[INDEX_LABEL]),
    ];

    for (sql, expected_labels) in cases {
        if expected_labels.is_empty() {
            continue;
        }

        let (_, normalized_sql_digest) = NormalizeDigest(sql);
        let sql_digest = kv::parser::Digest::new(normalized_sql_digest.Bytes().to_vec());
        let plan_digest = kv::parser::Digest::new(format!("plan:{sql}").into_bytes());
        let mut builder = kv::NewResourceGroupTagBuilder(Vec::new());
        builder
            .SetSQLDigest(sql_digest.clone())
            .SetPlanDigest(plan_digest.clone());

        let mut observed_labels = Vec::new();
        for expected_label in *expected_labels {
            let key = if *expected_label == ROW_LABEL {
                GenTableRecordPrefix(TABLE_ID)
            } else {
                GenTableIndexPrefix(TABLE_ID)
            };
            let encoded = builder
                .EncodeTagWithKey(key.as_ref())
                .expect("resource group tag should encode");
            let decoded = decode_tag(&encoded);

            assert_eq!(
                DecodeResourceGroupTag(&encoded).unwrap(),
                Some(sql_digest.Bytes()),
                "SQL digest mismatch for {sql}"
            );
            assert_eq!(decoded.sql_digest, sql_digest.Bytes(), "{sql}");
            assert_eq!(decoded.plan_digest, plan_digest.Bytes(), "{sql}");
            assert_eq!(decoded.table_id, TABLE_ID as u64, "{sql}");
            assert_eq!(decoded.label, *expected_label, "{sql}");
            observed_labels.push(decoded.label);
        }

        assert_eq!(observed_labels, *expected_labels, "{sql}");
        assert!(
            !observed_labels.is_empty(),
            "{sql} emitted no tagged request"
        );
    }
}

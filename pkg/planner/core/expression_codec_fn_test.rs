// Copyright 2026 AsterSQL.
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

use std::collections::BTreeMap;

use crate::{CodecColumn, CodecIndex, CodecRow, CodecTable, Datum, TiDBCodecFuncHelper};

fn table() -> CodecTable {
    CodecTable {
        id: 42,
        name: "orders".into(),
        columns: vec![
            CodecColumn {
                id: 1,
                name: "id".into(),
                primary_key: true,
                unsigned: false,
            },
            CodecColumn {
                id: 2,
                name: "note".into(),
                primary_key: false,
                unsigned: false,
            },
        ],
        indices: vec![CodecIndex {
            id: 7,
            name: "note_idx".into(),
            column_ids: vec![2],
            unique: false,
        }],
        common_handle: false,
        partitions: BTreeMap::from([(99, "p0".into())]),
    }
}

#[test]
fn decode_accepts_go_hex_input_and_returns_go_json_shape() {
    let helper = TiDBCodecFuncHelper;
    let tables = [table()];

    assert_eq!(
        helper.decodeKeyFromString("743432", &tables).unwrap(),
        r#"{"table_id":42}"#
    );
    assert_eq!(
        helper.decodeKeyFromString("7434325f7239", &tables).unwrap(),
        r#"{"id":9,"table_id":"42"}"#
    );
    assert_eq!(
        helper
            .decodeKeyFromString("7439395f69375f616263", &tables)
            .unwrap(),
        r#"{"index_id":7,"index_vals":{"note":"abc"},"partition_id":99,"table_id":42}"#
    );
}

#[test]
fn datum_json_string_uses_json_escaping() {
    let helper = TiDBCodecFuncHelper;
    assert_eq!(
        helper.datumToJSONObject(&Datum::String("a\\b\n\t\u{0001}\"".into())),
        r#""a\\b\n\t\u0001\"""#
    );
}

#[test]
fn non_unique_index_key_requires_and_contains_the_handle() {
    let helper = TiDBCodecFuncHelper;
    let table = table();
    let index = &table.indices[0];
    let row_without_handle = CodecRow {
        values: BTreeMap::from([(2, Datum::String("abc".into()))]),
    };
    assert_eq!(
        helper
            .encodeIndexKeyFromRow(&table, index, &row_without_handle)
            .unwrap_err(),
        "column id is not an integer handle"
    );

    let row = CodecRow {
        values: BTreeMap::from([(1, Datum::Int(9)), (2, Datum::String("abc".into()))]),
    };
    assert_eq!(
        helper.encodeIndexKeyFromRow(&table, index, &row).unwrap(),
        b"t42_i7_abc|9"
    );
}

#[test]
fn table_partition_lookup_matches_go_empty_and_error_branches() {
    let helper = TiDBCodecFuncHelper;
    let tables = [table()];

    assert_eq!(
        helper
            .findCommonOrPartitionedTable(&tables, "ORDERS(p0)")
            .unwrap()
            .1,
        Some(99)
    );
    assert_eq!(
        helper
            .findCommonOrPartitionedTable(&tables, "orders()")
            .unwrap()
            .1,
        None
    );
    assert_eq!(
        helper
            .findCommonOrPartitionedTable(&tables, "orders(missing)")
            .unwrap_err(),
        "partition missing does not exist"
    );
}

#[test]
fn common_handle_uses_primary_index_order_and_reports_missing_values() {
    let helper = TiDBCodecFuncHelper;
    let mut table = table();
    table.common_handle = true;
    table.indices.push(CodecIndex {
        id: 1,
        name: "PRIMARY".into(),
        column_ids: vec![2, 1],
        unique: true,
    });
    let row = CodecRow {
        values: BTreeMap::from([(1, Datum::Int(9)), (2, Datum::String("abc".into()))]),
    };
    assert_eq!(
        helper.encodeHandleFromRow(&table, &row).unwrap(),
        b"t42_rabc|9"
    );

    let missing = CodecRow {
        values: BTreeMap::from([(1, Datum::Int(9))]),
    };
    assert_eq!(
        helper.buildHandle(&table, &missing).unwrap_err(),
        "column 2 is missing"
    );
}

#[test]
fn decode_partition_and_index_keys_return_deterministic_json() {
    let helper = TiDBCodecFuncHelper;
    let tables = [table()];

    assert_eq!(
        helper.decodeKeyFromString("743939", &tables).unwrap(),
        r#"{"partition_id":99,"table_id":42}"#
    );
    assert_eq!(
        helper
            .decodeKeyFromString("7439395f69375f6162637c39", &tables)
            .unwrap(),
        r#"{"index_id":7,"index_vals":{"note":"abc"},"partition_id":99,"table_id":42}"#
    );
    assert_eq!(
        helper
            .decodeKeyFromString("7437375f69335f787c79", &tables)
            .unwrap(),
        r#"{"index_id":3,"index_vals":"x, y","table_id":77}"#
    );
}

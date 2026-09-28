// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// KV → SQL 解码器（TableKVDecoder）的单元测试。
//
// 覆盖聚簇主键（common handle）与整数主键两种表结构下，
// 从行记录（record key，键中含 `_r`）与索引键（index key，含 `_i`）
// 解码 Handle（行标识）并枚举原始索引键的行为。

use encode::{Column, Datum, SessionOptions};

use crate::*;

/// 聚簇主键表：主键列参与 common handle，非主键索引应能被 IterRawIndexKeys 枚举。
#[test]
fn TestIterRawIndexKeysClusteredPK() {
    let table = TableDefinition {
        common_handle: true,
        columns: vec![
            Column {
                name: "id".into(),
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "tenant".into(),
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "v".into(),
                ..Default::default()
            },
        ],
        indices: vec![
            IndexDefinition {
                id: 1,
                columns: vec![0, 1],
                primary: true,
                unique: true,
            },
            IndexDefinition {
                id: 2,
                columns: vec![2],
                primary: false,
                unique: false,
            },
        ],
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    let raw = encodeDatumList(&[
        Datum::Int(7),
        Datum::Bytes(b"tenant-a".to_vec()),
        Datum::String("value".into()),
    ]);
    let mut keys = Vec::new();
    let handle = Handle::Common(vec![
        DatumKey::Int(7),
        DatumKey::Bytes(b"tenant-a".to_vec()),
    ]);
    // 仅收集非主键索引键；聚簇主键本身不作为独立索引键迭代。
    decoder
        .IterRawIndexKeys(&handle, &raw, |key| {
            keys.push(key.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        tablecodec::DecodeKeyHead(tablecodec::kv::Key(keys[0].clone())).unwrap(),
        (1, 2, false)
    );
    assert_eq!(
        decoder.DecodeHandleFromIndex(2, &keys[0], b"").unwrap(),
        handle
    );
}

/// 整数主键（pk_is_handle）表：验证 Name、从行键解码 Handle、解码行数据，
/// 以及从二级索引键反向还原 Handle。
#[test]
fn TestIterRawIndexKeysIntPK() {
    let table = TableDefinition {
        pk_is_handle: true,
        columns: vec![
            Column {
                name: "id".into(),
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "v".into(),
                ..Default::default()
            },
        ],
        indices: vec![IndexDefinition {
            id: 3,
            columns: vec![1],
            primary: false,
            unique: false,
        }],
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "schema.t", &SessionOptions::default()).unwrap();
    assert_eq!(decoder.Name(), "schema.t");
    let row_key = tablecodec::EncodeRowKeyWithHandle(1, Box::new(tablecodec::kv::IntHandle(42))).0;
    assert_eq!(
        decoder.DecodeHandleFromRowKey(&row_key).unwrap(),
        Handle::Int(42)
    );
    let raw = encodeDatumList(&[Datum::Int(42), Datum::String("v".into())]);
    let (row, _) = decoder.DecodeRawRowData(&Handle::Int(42), &raw).unwrap();
    assert_eq!(row, vec![Datum::Int(42), Datum::String("v".into())]);
    let mut keys = Vec::new();
    decoder
        .IterRawIndexKeys(&Handle::Int(42), &raw, |key| {
            keys.push(key.to_vec());
            Ok(())
        })
        .unwrap();
    // 索引键中应能还原出整数 Handle。
    assert_eq!(
        decoder.DecodeHandleFromIndex(3, &keys[0], b"").unwrap(),
        Handle::Int(42)
    );
}

/// 解码失败时保留 Go `DecodeRawRowDataAsStr` 的 SQL 注释形错误文本。
#[test]
fn decode_raw_row_data_as_str_formats_error_like_go() {
    let decoder =
        NewTableKVDecoder(TableDefinition::default(), "t", &SessionOptions::default()).unwrap();
    let message = decoder.DecodeRawRowDataAsStr(&Handle::Int(1), b"invalid");
    assert!(message.starts_with("/* ERROR: "));
    assert!(message.ends_with(" */"));
}

#[test]
fn decode_raw_row_data_as_str_uses_go_datum_format() {
    let table = TableDefinition {
        columns: vec![
            Column::default(),
            Column::default(),
            Column {
                column_type: encode::ColumnType::Bytes,
                ..Default::default()
            },
            Column::default(),
        ],
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    let raw = encodeDatumList(&[
        Datum::Int(7),
        Datum::String("value".into()),
        Datum::Bytes(b"raw".to_vec()),
        Datum::Null,
    ]);

    assert_eq!(
        decoder.DecodeRawRowDataAsStr(&Handle::Int(1), &raw),
        "(7, \"value\", raw, NULL)"
    );
}

/// Go `DecodeRawRowData` returns the decoded row map, keyed by the 1-based
/// column IDs, rather than a map containing only generated columns.
#[test]
fn decode_raw_row_data_returns_all_encoded_columns() {
    let table = TableDefinition {
        columns: vec![
            Column {
                name: "a".into(),
                ..Default::default()
            },
            Column {
                name: "b".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    let raw = encodeDatumList(&[Datum::Int(7), Datum::String("value".into())]);

    let (_, decoded) = decoder.DecodeRawRowData(&Handle::Int(1), &raw).unwrap();

    assert_eq!(decoded.get(&1), Some(&Datum::Int(7)));
    assert_eq!(decoded.get(&2), Some(&Datum::String("value".into())));
}

/// Go `index.FetchValues` rejects invalid index metadata instead of silently
/// producing a key from only the in-range columns.
#[test]
fn iter_raw_index_keys_rejects_an_out_of_range_index_column() {
    let table = TableDefinition {
        columns: vec![Column {
            name: "a".into(),
            ..Default::default()
        }],
        indices: vec![IndexDefinition {
            id: 1,
            columns: vec![1],
            primary: false,
            unique: false,
        }],
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    let raw = encodeDatumList(&[Datum::Int(7)]);

    let error = decoder
        .IterRawIndexKeys(&Handle::Int(1), &raw, |_| Ok(()))
        .unwrap_err();

    assert!(error.contains("column index 1 out of range"));
}

/// TiDB record values may omit an integer primary-key column because its value
/// is carried by the record-key handle. Go restores it during row decoding.
#[test]
fn decode_raw_row_data_restores_integer_primary_key_from_handle() {
    let table = TableDefinition {
        columns: vec![
            Column {
                name: "id".into(),
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "v".into(),
                ..Default::default()
            },
        ],
        pk_is_handle: true,
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    let raw = encodeDatumList(&[Datum::Null, Datum::String("value".into())]);

    let (row, _) = decoder.DecodeRawRowData(&Handle::Int(42), &raw).unwrap();

    assert_eq!(row, vec![Datum::Int(42), Datum::String("value".into())]);
}

/// Missing non-handle columns use table defaults, matching
/// `tables.DecodeRawRowData` rather than remaining NULL.
#[test]
fn decode_raw_row_data_applies_column_defaults() {
    let table = TableDefinition {
        columns: vec![
            Column {
                name: "a".into(),
                ..Default::default()
            },
            Column {
                name: "b".into(),
                ..Default::default()
            },
        ],
        defaults: [(1, Datum::String("default".into()))].into_iter().collect(),
        ..Default::default()
    };
    let decoder = NewTableKVDecoder(table, "t", &SessionOptions::default()).unwrap();
    let raw = encodeDatumList(&[Datum::Int(7)]);

    let (row, _) = decoder.DecodeRawRowData(&Handle::Int(1), &raw).unwrap();

    assert_eq!(row, vec![Datum::Int(7), Datum::String("default".into())]);
}

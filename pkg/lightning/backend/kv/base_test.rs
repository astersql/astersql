// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// `base` 模块单元测试：覆盖 KV 转换失败日志格式与 Datum 转调试字符串。

use encode::Datum;

use crate::*;

/// 验证 `LogKVConvertFailed` 日志包含列名、下标、错误信息与原始行值。
#[test]
fn TestLogKVConvertFailed() {
    let encoder = crate::sql2kv_test::makeBaseEncoder(TableDefinition::default());
    let message =
        encoder.LogKVConvertFailed(&[Datum::String("bad".into())], 0, "c1", "out of range");
    assert!(message.contains("c1(0)"));
    assert!(message.contains("out of range"));
    assert!(message.contains("bad"));
}

/// 验证 `datumToValueStringForCastError` 对各 Datum 变体的可读字符串表示。
#[test]
fn TestDatumToValueStringForCastError() {
    assert_eq!(datumToValueStringForCastError(&Datum::Null), "NULL");
    assert_eq!(datumToValueStringForCastError(&Datum::Int(-42)), "-42");
    assert_eq!(
        datumToValueStringForCastError(&Datum::String("hello".into())),
        "\"hello\""
    );
    assert_eq!(
        datumToValueStringForCastError(&Datum::Bytes(vec![0x00, 0x01, 0x02])),
        "0x000102"
    );
}

#[test]
fn record_to_kv_attaches_go_comparable_row_id() {
    let mut encoder = crate::sql2kv_test::makeBaseEncoder(TableDefinition::default());
    let pairs = encoder.Record2KV(Vec::new(), &[], 42).unwrap();
    assert_eq!(pairs.RowID, vec![50]);
}

#[test]
fn row_array_marshaller_matches_go_sentinel_names() {
    assert_eq!(
        RowArrayMarshaller(&[Datum::MinNotNull, Datum::MaxValue]).MarshalLogArray(),
        vec![("min".into(), "-inf".into()), ("max".into(), "+inf".into()),]
    );
}

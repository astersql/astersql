// Copyright 2026 AsterSQL.

use crate::interface::Value;
use crate::query::ScanRow;

#[test]
fn scan_row_formats_float_values_like_database_sql() {
    let columns = vec![
        "positive".to_owned(),
        "negative".to_owned(),
        "nan".to_owned(),
    ];
    let row = vec![
        Value::F64(f64::INFINITY),
        Value::F64(f64::NEG_INFINITY),
        Value::F64(f64::NAN),
    ];

    let scanned = ScanRow(&columns, &row).expect("matching columns and values should scan");
    assert_eq!(scanned["positive"].Data, b"+Inf");
    assert_eq!(scanned["negative"].Data, b"-Inf");
    assert_eq!(scanned["nan"].Data, b"NaN");
}

#[test]
fn scan_row_preserves_null_empty_bytes_and_duplicate_column_semantics() {
    let columns = vec![
        "nullable".to_owned(),
        "nullable".to_owned(),
        "empty".to_owned(),
    ];
    let row = vec![
        Value::Null,
        Value::Bytes(b"later".to_vec()),
        Value::Bytes(Vec::new()),
    ];

    let scanned = ScanRow(&columns, &row).expect("matching columns and values should scan");
    assert_eq!(scanned["nullable"].Data, b"later");
    assert!(!scanned["nullable"].IsNull);
    assert!(scanned["empty"].Data.is_empty());
    assert!(!scanned["empty"].IsNull);
}

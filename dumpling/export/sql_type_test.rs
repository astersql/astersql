// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! SQL escaping remains here; CSV framing tests live in csvfile.

use crate::*;

#[test]
fn numeric_classification_and_raw_append_preserve_null() {
    for ty in ["DOUBLE PRECISION", "DECIMAL", "BOOL", "INT"] {
        assert!(dataTypeNumContains(ty));
    }
    assert!(!dataTypeNumContains("UNKNOWN"));
    let mut row = MakeRowReceiver(&["INT".into(), "BLOB".into(), "UNKNOWN".into()]);
    row.BindAddress(&mut [
        RawBytes(Some(b"1".to_vec())),
        RawBytes(None),
        RawBytes(Some(vec![])),
    ]);
    let mut raw = vec![RawBytes(Some(b"prefix".to_vec()))];
    row.appendRawBytes(&mut raw);
    assert_eq!(raw.len(), 4);
    assert_eq!(raw[1].as_opt(), Some(b"1".as_slice()));
    assert!(raw[2].as_opt().is_none());
    assert_eq!(raw[3].as_opt(), Some(b"".as_slice()));
}

#[test]
fn raw_receiver_refreshes_rows_and_preserves_unknown_types() {
    let mut receiver = MakeRowReceiver(&["UNKNOWN".into()]);
    receiver.BindAddress(&mut [RawBytes(Some(vec![255, 0]))]);
    assert_eq!(receiver.GetRawBytes()[0].0, Some(vec![255, 0]));
    receiver.BindAddress(&mut [RawBytes(None)]);
    assert!(receiver.GetRawBytes()[0].0.is_none());
}

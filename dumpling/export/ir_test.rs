// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

use crate::*;

#[derive(Default)]
struct BindingProbe {
    bind_calls: usize,
}

impl RowReceiver for BindingProbe {
    fn BindAddress(&mut self, _args: &mut [RawBytes]) {
        self.bind_calls += 1;
    }
}

#[test]
fn decode_from_rows_binds_receiver_before_scan_error() {
    let mut rows = Rows::new(vec!["value".into()], vec![]);
    let mut args = vec![RawBytes(None)];
    let mut receiver = BindingProbe::default();

    let err = decodeFromRows(&mut rows, &mut args, &mut receiver).unwrap_err();

    assert_eq!(receiver.bind_calls, 1);
    assert!(rows.closed);
    assert_eq!(err.to_string(), "sql: Rows are closed");
}

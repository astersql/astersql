// Copyright 2026 AsterSQL.

use crate::importer::{NewMockImportKV_WriteEngineClient, NewMockImportKVClient};
use crate::stubs::{Context, Controller, Error};

#[test]
fn importer_stream_creation_propagates_recorded_error() {
    let ctrl = Controller::new();
    let client = NewMockImportKVClient(ctrl.clone());
    client
        .EXPECT()
        .WriteEngine(&(), &[])
        .ReturnError(Some(Error::new("write stream unavailable")));

    let err = match client.WriteEngine(Context::background(), vec![]) {
        Ok(_) => panic!("WriteEngine unexpectedly succeeded"),
        Err(err) => err,
    };
    assert_eq!(err.msg, "write stream unavailable");
    assert_eq!(ctrl.remaining(), 0);
}

#[test]
fn importer_stream_response_methods_propagate_recorded_error() {
    let ctrl = Controller::new();
    let stream = NewMockImportKV_WriteEngineClient(ctrl.clone());

    stream
        .EXPECT()
        .Header()
        .ReturnError(Some(Error::new("header unavailable")));
    assert_eq!(stream.Header().unwrap_err().msg, "header unavailable");

    stream
        .EXPECT()
        .CloseAndRecv()
        .ReturnError(Some(Error::new("close failed")));
    assert_eq!(stream.CloseAndRecv().unwrap_err().msg, "close failed");
    assert_eq!(ctrl.remaining(), 0);
}

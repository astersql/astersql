// Copyright 2026 AsterSQL.

use std::collections::HashMap;

use crate::tikv_handler::{
    NewTikvHandlerTool, PhysicalTable, Storage, UrlValues, get_handle, handle_mvcc_get_by_hex,
};
use crate::util::{HANDLE, HEX_KEY};

#[test]
fn invalid_integer_handle_is_traced_not_reclassified_as_bad_request() {
    let tool = NewTikvHandlerTool(Storage);
    let mut params = HashMap::new();
    params.insert(HANDLE.to_owned(), "not-an-integer".to_owned());

    let error = get_handle(&tool, &PhysicalTable, &params, &UrlValues)
        .err()
        .expect("invalid strconv input must fail");

    assert!(!error.is_bad_request());
}

#[test]
fn invalid_hex_key_is_traced_not_reclassified_as_bad_request() {
    let tool = NewTikvHandlerTool(Storage);
    let mut params = HashMap::new();
    params.insert(HEX_KEY.to_owned(), "xyz".to_owned());

    let error = handle_mvcc_get_by_hex(&tool, &params)
        .err()
        .expect("invalid hex input must fail");

    assert!(!error.is_bad_request());
}

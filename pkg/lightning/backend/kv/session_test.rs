// Copyright 2026 AsterSQL.

use crate::transaction;

#[test]
fn transaction_get_remains_unsupported_after_set() {
    let mut txn = transaction::default();
    txn.Set(b"key", b"value").unwrap();

    assert_eq!(txn.Get(b"key"), Err("key not exist".to_owned()));
}

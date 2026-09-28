// Copyright 2026 AsterSQL.

use crate::mock::New;
use crate::pd::NULL_KEYSPACE_ID;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn temp_prefixed_path_is_volatile_and_removed_on_close() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after the Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tidb-unistore-temp-parity-{nonce}"));

    let (client, _pd, _cluster) =
        New(&path, Vec::new(), NULL_KEYSPACE_ID, Vec::new()).expect("create unistore");
    assert!(path.exists());

    client.close().expect("close unistore");
    assert!(
        !path.exists(),
        "Go treats every tidb-unistore-temp* path as volatile"
    );
}

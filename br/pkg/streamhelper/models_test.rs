// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::client::NewMetaDataClient;
use crate::models::{NewTaskInfo, RangeKeyOf, RangesOf};
use crate::stubs::{EtcdKV, MemEtcd};

#[test]
fn range_key_preserves_arbitrary_start_key_bytes() {
    let start_key = [0xff, b'/', b'/', 0x00, 0x80];
    let mut expected = RangesOf("binary_task").into_bytes();
    expected.extend_from_slice(&start_key);

    assert_eq!(RangeKeyOf("binary_task", &start_key), expected);
}

#[test]
fn put_task_writes_binary_range_key_without_utf8_conversion() {
    let kv = Arc::new(MemEtcd::new());
    let client = NewMetaDataClient(kv.clone());
    let start_key = [0xff, 0x00, 0x80];
    let task = NewTaskInfo("binary_task").WithRange(&start_key, b"end");

    client.PutTask(&task).unwrap();

    let entries = kv.GetPrefix(&RangesOf("binary_task")).unwrap();
    assert_eq!(
        entries,
        vec![(RangeKeyOf("binary_task", &start_key), b"end".to_vec())]
    );
}

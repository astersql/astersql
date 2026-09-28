// Copyright 2026 AsterSQL.

use crate::mvcc::{Lock, MutationOp};
use crate::write::{DbWriter, MemoryWriteBackend};
use std::sync::Arc;

fn lock(op: MutationOp) -> Lock {
    Lock {
        primary: b"k".to_vec(),
        start_ts: 0x0102_0304_0506_0708,
        ttl: 100,
        op,
        value: b"v".to_vec(),
        for_update_ts: 0,
        min_commit_ts: 11,
        use_async_commit: false,
        secondaries: Vec::new(),
        rollback_ts: Vec::new(),
    }
}

#[test]
fn commit_user_meta_uses_go_little_endian_layout() {
    let backend = Arc::new(MemoryWriteBackend::new(0));
    let writer = DbWriter::new(Arc::clone(&backend));
    writer.open();
    let mut batch = writer.new_write_batch(0x0102_0304_0506_0708, 0x1112_1314_1516_1718);
    batch.commit(b"k".to_vec(), lock(MutationOp::Put));
    writer.write(batch).unwrap();

    assert_eq!(
        vec![
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13,
            0x12, 0x11,
        ],
        backend.versions(b"k")[0].user_meta
    );
}

#[test]
fn rollback_key_matches_go_extra_txn_status_encoding() {
    let backend = Arc::new(MemoryWriteBackend::new(0));
    let writer = DbWriter::new(Arc::clone(&backend));
    writer.open();
    let mut batch = writer.new_write_batch(0x0102_0304_0506_0708, 0);
    batch.rollback(b"key".to_vec(), false);
    writer.write(batch).unwrap();

    let mut expected = b"key".to_vec();
    expected.extend_from_slice(&(!0x0102_0304_0506_0708_u64).to_be_bytes());
    expected[0] = expected[0].wrapping_add(1);
    assert_eq!(1, backend.versions(&expected).len());
}

#[test]
fn lock_limit_counts_go_marshaled_header() {
    let backend = Arc::new(MemoryWriteBackend::new(16));
    let writer = DbWriter::new(backend);
    writer.open();
    let mut batch = writer.new_write_batch(1, 0);
    batch.prewrite(b"k".to_vec(), lock(MutationOp::Put));
    assert!(writer.write(batch).is_err());
}

#[test]
#[should_panic(expected = "invalid end key")]
fn delete_range_panics_for_empty_end_like_go() {
    let backend = Arc::new(MemoryWriteBackend::new(0));
    let writer = DbWriter::new(backend);
    writer.delete_range(b"a", b"").unwrap();
}

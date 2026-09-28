// Copyright 2026 AsterSQL.

use crate::{BootstrapWithSingleStore, Cluster, encode_bytes};
use std::sync::Arc;

#[test]
fn split_keys_uses_storage_key_order_like_go() {
    let manager = Arc::new(crate::tikv::mock_region::MockRegionManager::new(1, 0));
    let cluster = Cluster::new(Arc::clone(&manager));
    BootstrapWithSingleStore(&cluster).expect("bootstrap mock cluster");

    let regions = cluster
        .split_keys(
            b"a",
            b"z",
            2,
            &[b"c".to_vec(), b"d".to_vec(), b"a".to_vec(), b"b".to_vec()],
        )
        .expect("split keys");

    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].start_key, encode_bytes(b"c"));
}

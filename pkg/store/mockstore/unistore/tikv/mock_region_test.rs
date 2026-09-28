// Copyright 2026 AsterSQL.

use crate::mock_region::{ExternalTimestampError, MockPd, MockRegionManager};
use std::sync::Arc;

#[test]
fn calculate_split_keys_distributes_remainder_like_go() {
    let manager = MockRegionManager::new(1, 1024);
    let keys = [b"a", b"b", b"c", b"d", b"e"]
        .into_iter()
        .map(|key| key.to_vec())
        .collect::<Vec<_>>();

    assert_eq!(
        vec![b"c".to_vec(), b"e".to_vec()],
        manager.calculate_split_keys(&keys, b"a", b"z", 3)
    );
}

#[test]
fn external_timestamp_rejects_future_values_and_decreases() {
    let pd = MockPd::new(Arc::new(MockRegionManager::new(1, 1024)));

    assert_eq!(
        Err(ExternalTimestampError::GreaterThanGlobalTso),
        pd.set_external_timestamp(u64::MAX)
    );
    assert_eq!(Ok(()), pd.set_external_timestamp(1));
    assert_eq!(1, pd.get_external_timestamp());
    assert_eq!(
        Err(ExternalTimestampError::Decreasing),
        pd.set_external_timestamp(0)
    );
    assert_eq!(1, pd.get_external_timestamp());
}

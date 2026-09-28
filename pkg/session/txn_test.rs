// Copyright 2026 AsterSQL.

use crate::txn::{enableMockAutoIncIDRetry, mockAutoIncIDRetry};

#[test]
fn mock_auto_increment_retry_flag_matches_go_non_consuming_load() {
    enableMockAutoIncIDRetry();

    assert!(mockAutoIncIDRetry());
    assert!(
        mockAutoIncIDRetry(),
        "Go keeps the retry marker set until the test resets process state"
    );
}

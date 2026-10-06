// Copyright 2026 AsterSQL.

use crate::{DriverError, extractKeyErr};

#[test]
fn retryable_without_lock_detail_keeps_go_separator() {
    let error = extractKeyErr(Some(DriverError::Retryable("retry me".to_owned())))
        .expect_err("retryable errors must be propagated");

    assert_eq!(error, DriverError::Retryable("retry me ".to_owned()));
}

#[test]
fn shared_lock_lost_maps_to_non_retryable_sql_error() {
    let error = extractKeyErr(Some(DriverError::SharedLockLost {
        start_ts: 101,
        key: b"key".to_vec(),
    }))
    .expect_err("shared lock loss must be propagated");

    assert_eq!(
        error.to_string(),
        "[tikv:9015]Shared lock was lost during lock upgrade; transaction cannot continue, txnStartTS=101, key=6B6579"
    );
}

#[test]
fn second_shared_lock_upgrader_maps_to_non_retryable_deadlock() {
    let error = extractKeyErr(Some(DriverError::LockUpgradeConflict {
        key: b"key".to_vec(),
        owner_start_ts: 202,
    }))
    .expect_err("second upgrader must be aborted");

    assert_eq!(
        error,
        DriverError::Deadlock {
            lock_ts: 202,
            lock_key: b"key".to_vec(),
            retryable: false,
        }
    );
}

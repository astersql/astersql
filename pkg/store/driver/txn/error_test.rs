// Copyright 2026 AsterSQL.

use crate::{DriverError, extractKeyErr};

#[test]
fn retryable_without_lock_detail_keeps_go_separator() {
    let error = extractKeyErr(Some(DriverError::Retryable("retry me".to_owned())))
        .expect_err("retryable errors must be propagated");

    assert_eq!(error, DriverError::Retryable("retry me ".to_owned()));
}

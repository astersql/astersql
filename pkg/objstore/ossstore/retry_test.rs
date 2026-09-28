// Copyright 2026 AsterSQL.

use anyhow::Context;

use crate::OssRetryer;

#[derive(Debug)]
struct WrappedTransportError;

impl std::fmt::Display for WrappedTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("transport connection broken while reading response")
    }
}

impl std::error::Error for WrappedTransportError {}

#[test]
fn wrapped_connection_error_text_remains_retryable_like_go_sdk() {
    let error = Err::<(), _>(WrappedTransportError)
        .context("GetObject failed")
        .unwrap_err();

    assert!(OssRetryer::default().IsErrorRetryable(&error));
}

// Copyright 2026 AsterSQL.

use std::io::{self, Read};

use crate::test_storage::fill_random_from_reader;

struct FailingRandom;

impl Read for FailingRandom {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("entropy unavailable"))
    }
}

#[test]
fn random_generation_failure_is_propagated() {
    let err = fill_random_from_reader(&mut FailingRandom, &mut [0; 16]).unwrap_err();
    assert!(err.msg.contains("failed to generate test data"));
    assert!(err.msg.contains("entropy unavailable"));
}

#[test]
fn port_cb395494e6_storage_start_log_redacts_uri() {
    if let Some(case) = std::env::var_os("TASK20_STORAGE_LOG_CHILD") {
        let query = if case == "encoded" {
            "%7B%22tenant%22%3A%22%23context-value%22%7D"
        } else {
            r##"{"tenant":"#context-value"}"##
        };
        let mut cfg = crate::test_storage::TestStorageConfig {
            StorageURI: format!("s3:///?sse-kms-context={query}"),
            TestDataSize: 1024,
            CleanupOnSuccess: true,
            ..Default::default()
        };
        cfg.BackendOptions
            .S3
            .insert("session-token".into(), "session-token-value".into());
        cfg.BackendOptions.S3.insert(
            "sse-kms-context".into(),
            r##"{"tenant":"#context-value"}"##.into(),
        );
        let original = cfg.BackendOptions.S3.clone();
        crate::test_storage::RunTestStorage(cfg.clone()).unwrap();
        assert_eq!(cfg.BackendOptions.S3, original);
        return;
    }
    for case in ["encoded", "raw"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "test_storage_test::port_cb395494e6_storage_start_log_redacts_uri",
                "--nocapture",
            ])
            .env("TASK20_STORAGE_LOG_CHILD", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr.matches("Starting external storage test").count(), 1);
        for secret in ["context-value", "session-token-value"] {
            assert!(!stderr.contains(secret), "{stderr}");
        }
    }
}

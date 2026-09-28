// Copyright 2026 AsterSQL.

use std::process::Command;

use crate::{
    GcpDecryptClient, GcpDecryptResponse, GcpKmsConfig, MasterKeyKms, NewGcpKmsWithClient,
};

struct CloseErrorClient;

impl GcpDecryptClient for CloseErrorClient {
    fn Decrypt(
        &self,
        _ctx: &crate::Context,
        _name: &str,
        _ciphertext: &[u8],
        _ciphertext_crc32c: i64,
    ) -> Result<GcpDecryptResponse, String> {
        unreachable!("close regression does not decrypt")
    }

    fn Close(&mut self) -> Result<(), String> {
        Err("close failed".into())
    }
}

#[test]
fn close_error_is_reported_like_go() {
    const CHILD_ENV: &str = "ASTERSQL_GCP_CLOSE_ERROR_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        let config = MasterKeyKms {
            KeyId: "projects/p/locations/l/keyRings/r/cryptoKeys/k".into(),
            GcpKms: Some(GcpKmsConfig::default()),
            ..Default::default()
        };
        NewGcpKmsWithClient(config, CloseErrorClient)
            .unwrap()
            .Close();
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "gcp_test::close_error_is_reported_like_go",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("failed to close gcp kms client: close failed"),
        "close failure was not reported; stderr={stderr:?}"
    );
}

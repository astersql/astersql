// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn apply_distinguishes_missing_scheme_from_missing_host_like_go() {
    let mut target = backuppb::S3::default();

    let missing_scheme = S3BackendOptions {
        Endpoint: "example.com".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        missing_scheme.Apply(&mut target).unwrap_err().to_string(),
        "scheme not found in endpoint"
    );

    let missing_host = S3BackendOptions {
        Endpoint: "http:/bucket".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        missing_host.Apply(&mut target).unwrap_err().to_string(),
        "host not found in endpoint"
    );
}

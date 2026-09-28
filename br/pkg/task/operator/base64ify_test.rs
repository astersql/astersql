// Copyright 2026 AsterSQL.

use crate::base64ify::{Base64ify, encode_backend_for_test};
use crate::config::Base64ifyConfig;
use crate::stubs::Context;

#[test]
fn base64ify_cancelled_context_matches_go_noop_behavior() {
    let ctx = Context::Background();
    ctx.Cancel();

    Base64ify(
        ctx,
        Base64ifyConfig {
            StorageURI: "noop://".to_owned(),
            ..Base64ifyConfig::default()
        },
    )
    .expect("Go's noop storage constructor does not inspect the context");
}

#[test]
fn base64ify_rejects_unknown_storage_scheme() {
    let error = encode_backend_for_test("unknown://bucket/path")
        .expect_err("Go ParseBackend rejects unsupported schemes");

    assert!(error.to_string().contains("not support yet"));
}

#[test]
fn base64ify_uses_backuppb_protobuf_wire_format() {
    assert_eq!(encode_backend_for_test("noop://").unwrap(), "CgA=");
    assert_eq!(
        encode_backend_for_test("local:///tmp/base64ify").unwrap(),
        "EhAKDi90bXAvYmFzZTY0aWZ5"
    );
    assert_eq!(
        encode_backend_for_test("hdfs://host:8020/path").unwrap(),
        "MhcKFWhkZnM6Ly9ob3N0OjgwMjAvcGF0aA=="
    );
    assert_eq!(
        encode_backend_for_test("s3://bucket/prefix?force-path-style=true").unwrap(),
        "GhIaBmJ1Y2tldCIGcHJlZml4UAE="
    );
    assert_eq!(
        encode_backend_for_test("gcs://bucket/prefix").unwrap(),
        "IhASBmJ1Y2tldBoGcHJlZml4"
    );
    assert_eq!(
        encode_backend_for_test("azure://container/prefix").unwrap(),
        "OhMSCWNvbnRhaW5lchoGcHJlZml4"
    );
}

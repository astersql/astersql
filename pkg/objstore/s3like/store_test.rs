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

#[test]
fn port_cb395494e6_context_flag_is_registered() {
    let mut flags = pflag::FlagSet::default();
    DefineS3Flags(&mut flags);
    flags
        .Set("s3.sse-kms-context", r#"{"key":"secret"}"#)
        .unwrap();
}

#[test]
fn port_cb395494e6_context_encoding_and_flags() {
    fn same_sentinel<T: std::error::Error + PartialEq + 'static>(
        root: &(dyn std::error::Error + 'static),
        sentinel: &T,
    ) -> bool {
        root.downcast_ref::<T>() == Some(sentinel)
    }
    use base64::Engine as _;
    for (raw, normalized) in [
        ("", ""),
        ("{}", "{}"),
        (r#"{"\ud800":"\udc00"}"#, r#"{"�":"�"}"#),
        (
            " \n { \"tenant\" : \"tenant-a\", \"purpose\":\"backup\" } \t",
            r#"{"purpose":"backup","tenant":"tenant-a"}"#,
        ),
        (
            r#"{"租户":"租户一","emoji":"😀","escaped":"\u4e2d"}"#,
            r#"{"emoji":"😀","escaped":"中","租户":"租户一"}"#,
        ),
        (
            r#"{"key":"a+b / = & \" \\ \n"}"#,
            r#"{"key":"a+b / = \u0026 \" \\ \n"}"#,
        ),
    ] {
        let mut flags = pflag::FlagSet::default();
        DefineS3Flags(&mut flags);
        flags.Set("s3.sse-kms-context", raw).unwrap();
        flags.Set("s3.sse", "aws:kms").unwrap();
        let mut options = S3BackendOptions::default();
        options.ParseFromFlags(&flags).unwrap();
        let mut backend = backuppb::S3::default();
        for _ in 0..2 {
            options.Apply(&mut backend).unwrap();
            assert_eq!(
                backend.SseKmsEncryptionContext,
                base64::engine::general_purpose::STANDARD.encode(normalized)
            );
            assert_eq!(backend.Sse, "aws:kms");
            assert!(backend.SseKmsKeyId.is_empty());
        }
    }
    for raw in [
        " \n\t",
        "null",
        "[]",
        r#""secret-value""#,
        r#"{"secret-value":1}"#,
        r#"{"secret-value":null}"#,
        r#"{"secret-value":true}"#,
        r#"{"secret-value":{}}"#,
        r#"{"secret-value":[]}"#,
        r#"{"key":"secret-value","key":"other"}"#,
        r#"{"key":"secret-value","\u006bey":"other"}"#,
        r#"{"key":"secret-value""#,
        r#"{"key":"secret-value"}{}"#,
        r#"{"key":"secret-value"}garbage"#,
    ] {
        let error = encode_sse_kms_context(raw.as_bytes()).unwrap_err();
        assert!(same_sentinel(
            error.root_cause(),
            &*berrors::ErrStorageInvalidConfig
        ));
        assert!(
            error
                .root_cause()
                .to_string()
                .contains("BR:ExternalStorage:ErrStorageInvalidConfig")
        );
        let error = error.to_string();
        assert!(error.contains("sse-kms-context"));
        assert!(!error.contains("secret-value"));
    }
    assert!(encode_sse_kms_context(b"{\"key\":\"secret-value\xff\"}").is_err());
}

#[test]
fn port_cb395494e6_aws_context_requires_explicit_kms() {
    for (sse, key, provider, invalid) in [
        ("", "", "", true),
        ("", "test-key", "aws", true),
        ("AES256", "", "", true),
        ("AES256", "test-key", "aws", true),
        ("", "test-key", "ceph", false),
        ("AES256", "", "ceph", false),
        ("aws:kms", "", "aws", false),
        ("aws:kms", "test-key", "", false),
        ("aws:kms:dsse", "test-key", "aws", false),
    ] {
        let options = S3BackendOptions {
            Sse: sse.into(),
            SseKmsKeyID: key.into(),
            Provider: provider.into(),
            SseKmsContext: r#"{"tenant":"tenant-a"}"#.into(),
            ..Default::default()
        };
        let mut backend = backuppb::S3::default();
        let result = options.Apply(&mut backend);
        assert_eq!(result.is_err(), invalid);
        if let Err(error) = result {
            assert!(!error.to_string().contains("tenant-a"));
        } else {
            assert_eq!(
                (
                    backend.Sse.as_str(),
                    backend.SseKmsKeyId.as_str(),
                    backend.Provider.as_str()
                ),
                (sse, key, provider)
            );
        }
    }
}

// Copyright 2026 AsterSQL.

use crate::flags::{BackendOptions, FlagSet, define_flags, hidden_flags_for_stream};

#[test]
#[should_panic(expected = "flag redefined: s3.endpoint")]
fn duplicate_flag_registration_panics_like_go_pflag() {
    let mut flags = FlagSet::default();
    define_flags(&mut flags);
    define_flags(&mut flags);
}

#[test]
fn s3_parsing_matches_go_normalization_and_defaults() {
    let mut flags = FlagSet::default();
    define_flags(&mut flags);
    flags.set("s3.endpoint", "https://s3.invalid//").unwrap();

    let mut options = BackendOptions::default();
    options.parse_from_flags(&flags).unwrap();

    // Go strings.TrimSuffix removes exactly one trailing slash.
    assert_eq!(options.s3.endpoint, "https://s3.invalid/");
    assert!(options.s3.force_path_style);
}

#[test]
fn s3_parse_error_preserves_go_assignment_order() {
    let mut flags = FlagSet::default();
    for (name, value) in [
        ("s3.endpoint", "endpoint/"),
        ("s3.region", "region"),
        ("s3.sse", "aws:kms"),
        ("s3.sse-kms-key-id", "key"),
        ("s3.acl", "private"),
    ] {
        flags.register(name, "");
        flags.set(name, value).unwrap();
    }

    let mut options = BackendOptions::default();
    let error = options.parse_from_flags(&flags).unwrap_err();

    assert!(error.to_string().contains("s3.storage-class"));
    assert_eq!(options.s3.endpoint, "endpoint");
    assert_eq!(options.s3.sse, "aws:kms");
    assert_eq!(options.s3.sse_kms_key_id, "key");
    assert_eq!(options.s3.acl, "private");
    assert!(!options.s3.force_path_style);
}

#[test]
fn hiding_stream_flags_ignores_missing_flags_like_go() {
    let mut flags = FlagSet::default();
    flags.register("azblob.account-key", "");

    hidden_flags_for_stream(&mut flags).unwrap();

    assert!(flags.is_hidden("azblob.account-key"));
}

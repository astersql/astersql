// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

use crate::CIStr;

#[test]
fn cistr_new_uses_unicode_lowercase_like_go() {
    let value = CIStr::new("ÄBC");

    assert_eq!(value.O, "ÄBC");
    assert_eq!(value.L, "äbc");
}

#[test]
fn cistr_deserializes_legacy_single_string_like_go() {
    let value: CIStr = serde_json::from_str("\"ÄBC\"").unwrap();

    assert_eq!(value.O, "ÄBC");
    assert_eq!(value.L, "äbc");
}

// Copyright 2026 AsterSQL.

use super::join_version::is_optimized_version;

#[test]
fn optimized_version_uses_go_simple_unicode_lowercase_mapping() {
    // Go's unicode.ToLower maps U+0130 to the single rune `i`, while Rust's
    // full lowercase mapping expands it to `i` followed by U+0307.
    assert!(is_optimized_version("optİmized"));
    assert!(!is_optimized_version("optI\u{0307}mized"));
}

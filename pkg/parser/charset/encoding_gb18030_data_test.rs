// Copyright 2026 AsterSQL.

use crate::encoding_gb18030_data::gb18030_case;

/// Go strings.ToUpperSpecial uses one-rune Unicode simple mappings outside the
/// GB18030 override table; it must not apply Rust's multi-character expansion.
#[test]
fn gb18030_case_fallback_uses_go_simple_case_mapping() {
    assert_eq!(gb18030_case().to_upper("straße"), "STRAßE");
    assert_eq!(gb18030_case().to_upper("\u{1f80}"), "\u{1f88}");
    assert_eq!(gb18030_case().to_lower("\u{130}"), "i");
}

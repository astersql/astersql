// Copyright 2026 AsterSQL.

use super::*;

/// Go strings.ToUpperSpecial/ToLowerSpecial fall back to one-rune Unicode
/// mappings; Rust's standard string casing may expand one scalar into several.
#[test]
fn gbk_case_fallback_uses_go_simple_unicode_mapping() {
    let gbk = FindEncoding(CharsetGBK);

    assert_eq!(gbk.ToUpper("\u{00df}"), "\u{00df}");
    assert_eq!(gbk.ToUpper("\u{1f80}"), "\u{1f88}");
    assert_eq!(gbk.ToLower("\u{0130}"), "i");
}

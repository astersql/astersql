// Copyright 2026 AsterSQL.

use crate::locale_format::FormatByLocale;

#[test]
fn format_by_locale_preserves_unicode_decimal_digits_after_ascii_prefix() {
    assert_eq!(
        FormatByLocale("1٢", "0", "en_US"),
        ("1٢".into(), true, Ok(()))
    );
}

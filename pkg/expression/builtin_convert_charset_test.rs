// Copyright 2026 AsterSQL.

use super::builtin_convert_charset_kernel::{
    CHARSET_BIG5, CHARSET_GBK, DecodeOptions, FunctionProperty, WarningContext,
    conversion_property, decode_binary, encode_to_binary,
};

#[test]
fn unsupported_charset_falls_back_to_binary_like_go_find_encoding() {
    let input = "中文";
    assert_eq!(
        encode_to_binary(input, CHARSET_BIG5).unwrap(),
        input.as_bytes()
    );

    let bytes = [0xff, 0x80];
    let mut warnings = WarningContext::default();
    assert_eq!(
        decode_binary(
            &bytes,
            CHARSET_BIG5,
            DecodeOptions::default(),
            &mut warnings
        )
        .unwrap()
        .unwrap(),
        bytes
    );
    assert!(warnings.warnings.is_empty());
}

#[test]
fn function_property_lookup_is_case_sensitive_like_go_map_lookup() {
    assert_eq!(conversion_property("sha2"), FunctionProperty::BinaryAware);
    assert_eq!(conversion_property("SHA2"), FunctionProperty::None);
}

#[test]
fn scalar_warning_returns_only_prefix_before_invalid_sequence() {
    let input = [b'a', 0xff, b'b'];
    let mut warnings = WarningContext::default();
    let result = decode_binary(
        &input,
        CHARSET_GBK,
        DecodeOptions {
            cannot_convert_as_warning: true,
            strict_mode: false,
        },
        &mut warnings,
    )
    .unwrap()
    .unwrap();

    assert_eq!(result, b"a");
    assert_eq!(warnings.warnings.len(), 1);
}

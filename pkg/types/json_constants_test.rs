// Copyright 2026 AsterSQL.

use crate::json_binary::json_constants::*;

#[test]
fn unknown_type_templates_match_go() {
    assert_eq!(unknownTypeCodeErrorMsg, "unknown type code: %d");
    assert_eq!(unknownTypeErrorMsg, "unknown type: %s");
}

#[test]
fn json_precedence_matches_go() {
    for (name, expected) in [
        ("BLOB", -1),
        ("BIT", -2),
        ("OPAQUE", -3),
        ("DATETIME", -4),
        ("TIME", -5),
        ("DATE", -6),
        ("BOOLEAN", -7),
        ("ARRAY", -8),
        ("OBJECT", -9),
        ("STRING", -10),
        ("INTEGER", -11),
        ("UNSIGNED INTEGER", -11),
        ("DOUBLE", -11),
        ("NULL", -12),
    ] {
        assert_eq!(json_type_precedence(name), Some(expected), "{name}");
    }
    for name in ["", "null", "TIMESTAMP", "UNKNOWN"] {
        assert_eq!(json_type_precedence(name), None);
    }
}

#[test]
fn standard_json_errors_match_go_codes_and_classes() {
    for (error, code) in [
        (ErrInvalidJSONText, 3140),
        (ErrInvalidJSONType, 3853),
        (ErrInvalidJSONTextInParam, 3141),
        (ErrInvalidJSONPath, 3143),
        (ErrInvalidJSONCharset, 3144),
        (ErrInvalidJSONData, 3069),
        (ErrInvalidJSONPathMultipleSelection, 3149),
        (ErrInvalidJSONContainsPathType, 3150),
        (ErrJSONDocumentNULLKey, 3158),
        (ErrJSONDocumentTooDeep, 3157),
        (ErrJSONObjectKeyTooLong, 8129),
        (ErrInvalidJSONPathArrayCell, 3165),
        (ErrUnsupportedSecondArgumentType, 8067),
    ] {
        assert_eq!(error.Code(), code);
        assert_eq!(dbterror::terror::ToSQLError(&error).Code, code as u16);
        assert_eq!(
            dbterror::terror::GetErrClass(&error),
            if code == 8129 {
                dbterror::terror::ClassTypes
            } else {
                dbterror::terror::ClassJSON
            }
        );
    }
}

#[test]
fn standard_json_errors_generate_messages_and_preserve_rust_kinds() {
    let error = ErrInvalidJSONCharset.GenWithStackByArgs(&["binary".into()]);
    assert_eq!(
        error.to_string(),
        "[json:3144]Cannot create a JSON value from a string with CHARACTER SET 'binary'."
    );
    let error = ErrJSONObjectKeyTooLong.GenWithStackByArgs(&[]);
    assert_eq!(
        error.to_string(),
        "[types:8129]TiDB does not yet support JSON objects with the key length >= 65536"
    );
    let error = JsonError::new(ErrInvalidJSONCharset, "charset failure");
    assert_eq!(error.kind(), JsonErrorKind::InvalidJsonCharset);
    assert_eq!(error.to_string(), "charset failure");
    assert!(std::ptr::eq(
        &*ErrInvalidJSONCharset,
        &*ErrInvalidJSONCharset
    ));
}

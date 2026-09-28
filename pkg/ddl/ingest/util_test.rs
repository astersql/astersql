// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::util::{
    ERR_FOUND_DUPLICATE_KEYS_ID, ErrorArgument, IndexInfo, IngestError, TableInfo,
    try_convert_to_key_exists_err,
};

fn index_info() -> IndexInfo {
    IndexInfo {
        name: "idx_email".to_owned(),
    }
}

fn table_info() -> TableInfo {
    TableInfo {
        schema: "accounts".to_owned(),
        name: "users".to_owned(),
    }
}

fn duplicate_error(arguments: Vec<ErrorArgument>) -> IngestError {
    IngestError::Terror {
        id: ERR_FOUND_DUPLICATE_KEYS_ID,
        arguments,
    }
}

#[test]
fn duplicate_key_conversion_matches_go_table_and_index_contract() {
    let converted = try_convert_to_key_exists_err(
        duplicate_error(vec![
            ErrorArgument::Bytes(b"encoded-key".to_vec()),
            ErrorArgument::Bytes(b"encoded-value".to_vec()),
        ]),
        &index_info(),
        &table_info(),
    );

    assert_eq!(
        converted,
        IngestError::KeyExists {
            key: b"encoded-key".to_vec(),
            value: b"encoded-value".to_vec(),
            index_name: "idx_email".to_owned(),
            // Go ddlutil.GenKeyExistsErr uses tblInfo.Name, not schema-qualified name.
            table_name: "users".to_owned(),
        }
    );
}

#[test]
fn wrapped_duplicate_uses_root_cause_like_errors_cause() {
    let converted = try_convert_to_key_exists_err(
        IngestError::Wrapped {
            message: "outer".to_owned(),
            source: Arc::new(duplicate_error(vec![
                ErrorArgument::Bytes(vec![1, 2]),
                ErrorArgument::Bytes(vec![3, 4]),
            ])),
        },
        &index_info(),
        &table_info(),
    );

    assert!(matches!(converted, IngestError::KeyExists { .. }));
}

#[test]
fn non_matching_error_shapes_are_returned_unchanged() {
    let cases = [
        IngestError::Other("other".to_owned()),
        IngestError::Terror {
            id: ERR_FOUND_DUPLICATE_KEYS_ID + 1,
            arguments: vec![],
        },
        duplicate_error(vec![ErrorArgument::Bytes(vec![1])]),
        duplicate_error(vec![
            ErrorArgument::Text("not bytes".to_owned()),
            ErrorArgument::Bytes(vec![2]),
        ]),
        duplicate_error(vec![
            ErrorArgument::Bytes(vec![1]),
            ErrorArgument::Integer(2),
        ]),
    ];

    for origin in cases {
        assert_eq!(
            try_convert_to_key_exists_err(origin.clone(), &index_info(), &table_info()),
            origin
        );
    }
}

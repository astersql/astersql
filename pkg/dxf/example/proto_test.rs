// Copyright 2026 AsterSQL.

use crate::{subtaskMeta, taskMeta};

#[test]
fn struct_fields_match_case_insensitively_and_last_value_wins() {
    let decoded =
        taskMeta::Unmarshal(br#"{"SUBTASK_COUNT":1,"subtask_count":2,"Subtask_Count":3}"#)
            .expect("Go encoding/json accepts folded struct field names");
    assert_eq!(3, decoded.SubtaskCount);

    let decoded = subtaskMeta::Unmarshal(br#"{"MESSAGE":"first","Message":"last"}"#)
        .expect("Go encoding/json accepts folded struct field names");
    assert_eq!("last", decoded.Message);
}

#[test]
fn malformed_string_encoding_is_replaced_like_go_json() {
    let decoded = subtaskMeta::Unmarshal(b"{\"message\":\"\xff\"}")
        .expect("Go encoding/json replaces invalid UTF-8 in strings");
    assert_eq!("\u{fffd}", decoded.Message);

    let decoded = subtaskMeta::Unmarshal(br#"{"message":"\ud800x\udc00"}"#)
        .expect("Go encoding/json replaces unpaired UTF-16 surrogates");
    assert_eq!("\u{fffd}x\u{fffd}", decoded.Message);
}

#[test]
fn null_keeps_the_go_struct_zero_value() {
    assert_eq!(
        taskMeta::default(),
        taskMeta::Unmarshal(b"null").expect("Go encoding/json accepts null for a struct")
    );
    assert_eq!(
        subtaskMeta::default(),
        subtaskMeta::Unmarshal(b" \n null\t").expect("surrounding JSON whitespace is accepted")
    );
}

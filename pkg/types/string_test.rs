// Copyright 2026 AsterSQL.

use super::string::{HackedStr, PlainStr, String as StringValue};

#[test]
fn wrappers_match_go_string_contract() {
    let plain = PlainStr("stable".to_owned());
    let hacked = HackedStr("buffer".to_owned());

    assert_eq!(plain.String(), "stable");
    assert_eq!(hacked.String(), "buffer");
}

#[test]
fn hacked_str_implements_errors_freeze_contract() {
    let hacked = HackedStr("buffer".to_owned());
    let frozen = astersql_errors::ErrorArg::from_hacked(&hacked);

    assert_eq!(
        frozen,
        astersql_errors::ErrorArg::String("buffer".to_owned())
    );
}

// Copyright 2026 AsterSQL.

use super::*;
use std::collections::HashSet;

#[test]
fn schema_table_keys_use_normalized_identifier_identity() {
    let mixed = newSchemaTableKey(CIString::New("TeSt"), CIString::New("TaBlE"));
    let lower = newSchemaTableKey(CIString::New("test"), CIString::New("table"));

    assert_eq!(mixed, lower);
    assert_eq!(HashSet::from([mixed, lower]).len(), 1);
}

#[test]
fn alias_keys_preserve_qualification_independently_of_schema_text() {
    let unqualified = newTableAliasKey(CIString::New("TaBlE"));
    let qualified = newQualifiedTableAliasKey(CIString::New(""), CIString::New("table"));

    assert_ne!(unqualified, qualified);
    assert_eq!(unqualified, newTableAliasKey(CIString::New("table")));
}

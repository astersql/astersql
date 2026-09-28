// Copyright 2026 AsterSQL.

use crate::parser::{newTable, parseIndexSQL, parseTableSQL};

#[test]
fn column_string_formats_sets_like_go_fmt() {
    let mut table = newTable();
    parseTableSQL(
        &mut table,
        "create table t(a int comment '[[set=first,second]]');",
    )
    .unwrap();

    let rendered = table.columns[0].String();
    assert!(rendered.contains("set: [first second]"), "{rendered}");
}

#[test]
fn missing_index_column_is_retained_as_go_nil_entry() {
    let mut table = newTable();
    parseTableSQL(&mut table, "create table t(a int);").unwrap();
    parseIndexSQL(&mut table, "create index i_missing on t(missing);").unwrap();

    assert!(table.indices.contains_key("missing"));
    assert!(table.String().contains("key->missing, value-><nil>"));
}

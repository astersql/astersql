// Copyright 2026 AsterSQL.

use crate::add_column::process_column_flags;
use crate::column::{ColumnKind, FieldType};

fn varchar(charset: &str, binary: bool) -> FieldType {
    FieldType {
        kind: ColumnKind::Varchar,
        flen: 32,
        decimal: 0,
        charset: charset.into(),
        collation: format!("{charset}_bin"),
        binary,
        unsigned: false,
        zerofill: false,
    }
}

#[test]
fn string_binary_flag_follows_charset_like_go() {
    let mut binary_charset = varchar("binary", false);
    process_column_flags(&mut binary_charset);
    assert!(binary_charset.binary);

    let mut text_charset = varchar("utf8mb4", true);
    process_column_flags(&mut text_charset);
    assert!(!text_charset.binary);
}

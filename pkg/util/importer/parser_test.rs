// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn comment_rules_match_go_split_and_append_semantics() {
    let mut table = Table::new();
    parse_table_sql(
        &mut table,
        "CREATE TABLE t (\
            repeated INT COMMENT '[[set=a,b;set=c,d]]', \
            extra_equals INT COMMENT '[[set=x=y]]', \
            reversed_markers INT COMMENT ']] ignored [[step=9]]'\
        )",
    )
    .unwrap();

    assert_eq!(
        table.find_column("repeated").unwrap().set,
        ["a", "b", "c", "d"]
    );
    assert!(table.find_column("extra_equals").unwrap().set.is_empty());
    assert_eq!(table.find_column("reversed_markers").unwrap().step, 1);
}

#[test]
fn parsing_a_second_table_replaces_go_column_slice() {
    let mut table = Table::new();
    parse_table_sql(&mut table, "CREATE TABLE first (old_col INT)").unwrap();
    parse_table_sql(&mut table, "CREATE TABLE second (new_col VARCHAR(8))").unwrap();

    assert_eq!(table.name, "second");
    assert_eq!(table.columns.len(), 1);
    assert!(table.find_column("old_col").is_none());
    assert!(table.find_column("new_col").is_some());
    assert_eq!(table.column_list, "`new_col`");
}

#[test]
fn quoted_comments_identifiers_and_index_order_match_go_ast() {
    let mut table = Table::new();
    parse_table_sql(
        &mut table,
        "CREATE TABLE t (`a``b` TEXT COMMENT 'it''s [[step=2]]', KEY idx (`a``b`(8) DESC))",
    )
    .unwrap();

    let column = table.find_column("a`b").unwrap();
    assert_eq!(column.comment, "it's [[step=2]]");
    assert_eq!(column.step, 2);
    assert_eq!(column.field_type.length, 65_535);
    assert_eq!(table.column_list, "`a``b`");
    assert!(table.unique_indices.contains("a`b"));
}

#[test]
fn mysql_blob_families_keep_parser_field_lengths() {
    let mut table = Table::new();
    parse_table_sql(
        &mut table,
        "CREATE TABLE t (a TINYBLOB, b BLOB, c MEDIUMBLOB, d LONGBLOB, e CHAR)",
    )
    .unwrap();

    let lengths = table
        .columns
        .iter()
        .map(|column| column.field_type.length)
        .collect::<Vec<_>>();
    assert_eq!(lengths, [255, 65_535, 16_777_215, 4_294_967_295, 1]);
}

#[test]
fn create_index_keeps_go_empty_and_missing_column_boundaries() {
    let mut table = Table::new();
    parse_table_sql(&mut table, "CREATE TABLE t (a INT)").unwrap();

    assert!(parse_index_sql(&mut table, "").is_ok());
    assert!(parse_index_sql(&mut table, "   ").is_err());
    parse_index_sql(&mut table, "CREATE INDEX idx ON t (missing)").unwrap();
    assert!(table.indices.contains_key("missing"));
    assert!(table.indices["missing"].is_none());
}

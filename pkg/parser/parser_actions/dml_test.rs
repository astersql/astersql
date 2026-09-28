// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn dml_delete_single_table_keeps_go_modifiers_and_returning() {
    let statement = Parser::default()
        .ParseOneStmt(
            "DELETE /*+ MEMORY_QUOTA(1 MB) */ LOW_PRIORITY QUICK IGNORE FROM t RETURNING a",
            "",
            "",
        )
        .unwrap();
    let delete = statement
        .as_any()
        .downcast_ref::<parser_ast::DeleteStmt>()
        .unwrap();

    assert_eq!(delete.Priority, 1);
    assert!(delete.Quick);
    assert!(delete.IgnoreErr);
    assert_eq!(delete.TableHints.len(), 1);
    assert_eq!(delete.Returning.len(), 1);
}

#[test]
fn dml_insert_and_replace_keep_go_hints_and_returning() {
    let insert = Parser::default()
        .ParseOneStmt(
            "INSERT /*+ MEMORY_QUOTA(1 MB) */ HIGH_PRIORITY INTO t VALUES (1) RETURNING a",
            "",
            "",
        )
        .unwrap();
    let insert = insert
        .as_any()
        .downcast_ref::<parser_ast::InsertStmt>()
        .unwrap();
    assert_eq!(insert.TableHints.len(), 1);
    assert_eq!(insert.Priority, 2);
    assert_eq!(insert.Returning.len(), 1);

    let replace = Parser::default()
        .ParseOneStmt(
            "REPLACE /*+ MEMORY_QUOTA(1 MB) */ LOW_PRIORITY INTO t VALUES (1)",
            "",
            "",
        )
        .unwrap();
    let replace = replace
        .as_any()
        .downcast_ref::<parser_ast::InsertStmt>()
        .unwrap();
    assert_eq!(replace.TableHints.len(), 1);
    assert_eq!(replace.Priority, 1);
}

#[test]
fn dml_load_and_import_keep_optional_values_and_allow_select_without_set() {
    let load = Parser::default()
        .ParseOneStmt(
            "LOAD DATA INFILE 'file.csv' FORMAT 'csv' INTO TABLE t CHARACTER SET utf8mb4",
            "",
            "",
        )
        .unwrap();
    let load = load
        .as_any()
        .downcast_ref::<parser_ast::LoadDataStmt>()
        .unwrap();
    assert_eq!(load.Format.as_deref(), Some("csv"));
    assert_eq!(load.Charset.as_deref(), Some("utf8mb4"));

    let import = Parser::default()
        .ParseOneStmt("IMPORT INTO t FROM SELECT * FROM s", "", "")
        .unwrap();
    let import = import
        .as_any()
        .downcast_ref::<parser_ast::ImportIntoStmt>()
        .unwrap();
    assert!(import.Select.is_some());
}

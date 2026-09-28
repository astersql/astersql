// Copyright 2026 AsterSQL.

use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser_ast as ast;

use crate::mock::mock_table_info;

fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE");
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST");
    Box::new(ast::CreateTableStmt {
        node_text: Default::default(),
        IfNotExists: create.IfNotExists,
        TemporaryKeyword: create.TemporaryKeyword,
        OnCommitDelete: create.OnCommitDelete,
        Table: create.Table.clone(),
        ReferTable: create.ReferTable.clone(),
        Cols: create.Cols.clone(),
        Constraints: create.Constraints.clone(),
        Options: create.Options.clone(),
        Partition: create.Partition.clone(),
        SplitIndex: create.SplitIndex.clone(),
        OnDuplicate: create.OnDuplicate,
        Select: None,
    })
}

#[test]
fn mock_table_info_uses_the_full_go_create_table_pipeline() {
    let statement = parse_create(
        "create table child (id bigint primary key clustered, parent_id bigint, \
         g bigint generated always as (parent_id + 1) stored, \
         unique key uk_parent(parent_id), \
         constraint fk_parent foreign key(parent_id) references parent(id), \
         constraint chk_parent check(parent_id > 0)) \
         charset=latin1 collate=latin1_bin comment='child'",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());

    let table: model::TableInfo =
        mock_table_info(&context, &statement, 42).expect("build mock table metadata");

    assert_eq!(table.ID, 42);
    assert_eq!(table.Charset, "latin1");
    assert_eq!(table.Collate, "latin1_bin");
    assert_eq!(table.Comment, "child");
    assert!(table.PKIsHandle);
    assert_eq!(table.Columns.len(), 3);
    assert_eq!(table.Indices.len(), 1);
    assert_eq!(table.ForeignKeys.len(), 1);
    assert_eq!(table.Constraints.len(), 1);
}

#[test]
fn mock_table_info_propagates_full_builder_validation_errors() {
    let statement = parse_create("create table duplicate_names (a int, A bigint)");
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());

    assert!(mock_table_info(&context, &statement, 1).is_err());
}

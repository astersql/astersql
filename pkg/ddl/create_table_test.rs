// Copyright 2026 AsterSQL.

use astersql_meta_metabuild as metabuild;
use astersql_parser_ast as ast;

use crate::BuildTableInfoFromAST;

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
fn shard_row_id_bits_matches_go_clustered_error_and_upper_bound() {
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let clustered =
        parse_create("create table t (id bigint primary key clustered) shard_row_id_bits = 4");
    let error = BuildTableInfoFromAST(&context, &clustered).expect_err("reject clustered row ID");
    assert_eq!(
        error.to_string(),
        "[ddl:8200]Unsupported shard_row_id_bits for table with primary key as row id"
    );

    let nonclustered = parse_create("create table t (id bigint) shard_row_id_bits = 64");
    let table = BuildTableInfoFromAST(&context, &nonclustered).expect("build table metadata");
    assert_eq!(table.ShardRowIDBits, 15);
    assert_eq!(table.MaxShardRowIDBits, 15);
}

#[test]
fn ttl_suboptions_require_a_ttl_definition_like_go() {
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    for sql in [
        "create table t (created_at timestamp) ttl_enable = 'on'",
        "create table t (created_at timestamp) ttl_job_interval = '1h'",
    ] {
        let statement = parse_create(sql);
        assert!(
            BuildTableInfoFromAST(&context, &statement).is_err(),
            "{sql}"
        );
    }
}

// Copyright 2026 AsterSQL.

// [`BuildTableInfoFromASTForTest`] 单元测试：与 DDL 入口构建结果一致。

use astersql_parser_ast as ast;
use std::convert::Infallible;

use crate::BuildTableInfoFromASTForTest;

/// helper 必须保持 Go 版本直接转发到 DDL 入口的结果。
#[test]
fn helper_builds_the_same_table_metadata_as_the_ddl_entry_point() {
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(
            "create table child (id bigint primary key, note varchar(32)) comment='helper'",
            "",
            "",
        )
        .expect("parse CREATE TABLE");
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST");

    let helper_table = BuildTableInfoFromASTForTest(create).expect("helper builds table metadata");
    let context = astersql_meta_metabuild::NewContext::<(), Infallible>(Vec::new());
    let direct_table =
        astersql_ddl::BuildTableInfoFromAST(&context, create).expect("DDL builds table metadata");

    assert_eq!(helper_table.Name, direct_table.Name);
    assert_eq!(helper_table.Columns.len(), direct_table.Columns.len());
    assert_eq!(helper_table.Indices.len(), direct_table.Indices.len());
    assert_eq!(helper_table.Columns[0].Name, direct_table.Columns[0].Name);
    assert_eq!(helper_table.Columns[1].Name, direct_table.Columns[1].Name);
    assert_eq!(helper_table.Comment, direct_table.Comment);
    assert_eq!(helper_table.ID, 0, "TableID remains uninitialized");
}

/// Go 注释明确约定 PartitionID 也保持未初始化。
#[test]
fn helper_leaves_partition_ids_uninitialized() {
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(
            "create table parent (id bigint) partition by range (id) (partition p0 values less than (10), partition p1 values less than maxvalue)",
            "",
            "",
        )
        .expect("parse CREATE TABLE");
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST");

    let table = BuildTableInfoFromASTForTest(create).expect("build partitioned table metadata");
    let partition = table.Partition.expect("partition metadata");
    assert_eq!(partition.Definitions.len(), 2);
    assert!(
        partition
            .Definitions
            .iter()
            .all(|definition| definition.ID == 0),
        "PartitionID remains uninitialized"
    );
}

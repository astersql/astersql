// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `GetTableInfoBySQL` 的 Aster 单元测试。
//
// 覆盖：聚簇主键补 PRIMARY 索引、非句柄主键不重复添加、非 CREATE TABLE 被拒绝。

use super::*;

#[test]
/// 聚簇 bigint 主键：应得到 PKIsHandle，并合成唯一的 PRIMARY 索引。
fn get_table_info_by_sql_builds_metadata_and_adds_pk_handle_index() {
    let mut parser = astersql_parser::New();
    let table = GetTableInfoBySQL(
        "create table test.t (id bigint primary key clustered)",
        &mut parser,
    )
    .expect("CREATE TABLE should build real table metadata");

    assert_eq!(table.Name.O, "t");
    assert_eq!(table.Columns.len(), 1);
    assert_eq!(table.Columns[0].Name.O, "id");
    assert!(table.PKIsHandle);
    assert_eq!(table.Indices.len(), 1);
    let primary = &table.Indices[0];
    assert_eq!(primary.Name.O, "PRIMARY");
    assert!(primary.Primary);
    assert!(primary.Unique);
    assert_eq!(primary.State, astersql_meta_model::StatePublic);
    assert_eq!(primary.Tp, astersql_meta_model::ast::IndexType::Btree);
    assert_eq!(primary.Columns.len(), 1);
    assert_eq!(
        primary.Columns[0].Length,
        astersql_meta_model::types::UnspecifiedLength
    );
}

#[test]
/// 非句柄主键（如 varchar PK）：依赖 AST 已有 PRIMARY，不再额外合成。
fn get_table_info_by_sql_does_not_add_index_for_non_handle_primary_key() {
    let mut parser = astersql_parser::New();
    let table = GetTableInfoBySQL(
        "create table t (a varchar(32), primary key (a))",
        &mut parser,
    )
    .expect("CREATE TABLE should be accepted");

    assert!(!table.PKIsHandle);
    assert_eq!(table.Indices.len(), 1);
    assert!(table.Indices[0].Primary);
}

#[test]
/// SELECT 等非建表语句必须失败，错误信息包含原始 SQL。
fn get_table_info_by_sql_rejects_non_create_table_statement() {
    let mut parser = astersql_parser::New();
    let error = GetTableInfoBySQL("select 1", &mut parser)
        .expect_err("non-CREATE TABLE SQL must be rejected");

    assert_eq!(error.to_string(), "get table info from sql select 1 failed");
}

// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// DDL 集成校验相关的 Aster 单元测试。
//
// 覆盖建表 AST 构建、索引部件数上限、OPTIMIZE TABLE 不支持，以及
// `rand()` 默认值在 CREATE 允许、在 ALTER ADD COLUMN 拒绝等边界。

use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser_ast as ast;

use crate::BuildTableInfoFromAST;
use crate::add_column::{AddColumnError, ColumnDefinition, create_new_column};
use crate::column::{
    ColumnKind, ColumnPosition, DefaultValue, FieldType, TableInfo as ColumnTableInfo,
};
use crate::executor::{ExecutorError, validate_optimize_table};
use crate::index::{ColumnInfo, ColumnType, ColumnarIndexType, IndexError, build_index_columns};

/// 解析单条 CREATE TABLE SQL，克隆为独立的 `CreateTableStmt`（去掉 Select 子句）。
fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE");
    let create = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST");
    // 手工拷贝字段，避免持有 parser 内部借用；Select 置空以聚焦建表定义。
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

/// 从 CREATE TABLE SQL 构建 `TableInfo`（经 metabuild 上下文与 AST）。
fn build(sql: &str) -> Result<model::TableInfo, astersql_parser::errors::Error> {
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    BuildTableInfoFromAST(&context, &parse_create(sql))
}

/// `double(0,0)` 宽度/精度非法，建表应失败。
#[test]
fn rejects_double_with_zero_width_and_scale() {
    assert!(build("create table t(a double(0,0))").is_err());
}

/// 索引部件超过 16 列时，在列查找前即返回 TooManyKeyParts。
#[test]
fn rejects_more_than_sixteen_index_parts_before_column_lookup() {
    // 构造 17 个整型列及其索引规格，触发最大 16 部件限制。
    let columns = (0..17)
        .map(|id| ColumnInfo {
            id,
            name: format!("c{id}"),
            column_type: ColumnType::Int,
            charset_max_bytes: 1,
            generated: false,
            stored: false,
            hidden: false,
            nullable: true,
            primary_key: false,
            index_flags: 0,
            generated_dependencies: Default::default(),
        })
        .collect::<Vec<_>>();
    let specifications = (0..17)
        .map(|id| (format!("c{id}"), None))
        .collect::<Vec<_>>();
    assert_eq!(
        build_index_columns(&columns, &specifications, ColumnarIndexType::None),
        Err(IndexError::TooManyKeyParts {
            actual: 17,
            maximum: 16
        })
    );
}

/// OPTIMIZE TABLE 明确不支持，校验器返回 Unsupported。
#[test]
fn optimize_table_is_explicitly_unsupported() {
    assert_eq!(
        validate_optimize_table(),
        Err(ExecutorError::Unsupported(
            "OPTIMIZE TABLE is not supported".into()
        ))
    );
}

/// CREATE 允许 `default (rand())`，但 ALTER ADD COLUMN 因不安全默认函数拒绝。
#[test]
fn rand_default_is_allowed_on_create_but_rejected_on_alter_add() {
    assert!(build("create table t(a double default (rand()))").is_ok());
    let definition = ColumnDefinition {
        name: "a".into(),
        field_type: FieldType {
            kind: ColumnKind::Integer,
            ..FieldType::integer()
        },
        constraints: Vec::new(),
        default_value: Some(DefaultValue::Expression("rand()".into())),
        comment: String::new(),
        generated: None,
    };
    assert_eq!(
        create_new_column(
            &ColumnTableInfo::new(1, "t"),
            &definition,
            &ColumnPosition::None,
            false,
            true,
        ),
        Err(AddColumnError::UnsafeDefaultFunction("rand".into()))
    );
}

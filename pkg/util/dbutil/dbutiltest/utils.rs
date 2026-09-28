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

// dbutil 测试工具：由 CREATE TABLE SQL 构建 `TableInfo` 元数据。
//
// 对整数聚簇主键（`PKIsHandle`）会补一条合成 PRIMARY 索引，与 Go 侧 TableInfo
// 把行句柄（row handle）当作主键的表示方式对齐。

use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser as parser;
use astersql_parser_ast as ast;

/// Returns table information for one CREATE TABLE statement.
/// 解析单条 CREATE TABLE，经 metabuild/DDL 构建 `TableInfo`；非建表语句报错。
///
/// 当 `PKIsHandle` 为真时追加 PRIMARY 索引列，便于测试路径读取“主键即句柄”语义。
pub fn GetTableInfoBySQL(
    create_table_sql: &str,
    parser: &mut parser::Parser,
) -> Result<model::TableInfo, parser::errors::Error> {
    let statement = parser.ParseOneStmt(create_table_sql, "", "")?;
    let create_table = statement
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .ok_or_else(|| {
            parser::errors::New(format!("get table info from sql {create_table_sql} failed"))
        })?;

    // 空依赖上下文即可从 AST 构建表信息。
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let mut table = astersql_ddl::BuildTableInfoFromAST(&context, create_table)?;

    // Go adds a synthetic PRIMARY index for an integer clustered primary key,
    // because the canonical TableInfo represents that key as the row handle.
    // 整数聚簇主键在 TableInfo 中体现为行句柄，需补合成 PRIMARY 索引条目。
    if table.PKIsHandle {
        table.Indices.push(model::IndexInfo {
            Name: ast::NewCIStr("PRIMARY"),
            Primary: true,
            State: model::StatePublic,
            Unique: true,
            Tp: model::ast::IndexType::Btree,
            Columns: vec![model::IndexColumn {
                Name: table.GetPkName(),
                Length: model::types::UnspecifiedLength,
                ..Default::default()
            }],
            ..Default::default()
        });
    }

    Ok(table)
}

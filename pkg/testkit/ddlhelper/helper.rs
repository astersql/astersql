// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DDL 测试辅助：从 CREATE TABLE AST 构建 `model.TableInfo`。
//
// 转发到 `ddl::BuildTableInfoFromAST`；TableID / PartitionID 保持未初始化，
// 与 Go 测试辅助语义一致。

use std::convert::Infallible;

use astersql_ddl as ddl;
use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser as parser;
use astersql_parser_ast as ast;

// BuildTableInfoFromASTForTest builds model.TableInfo from a SQL statement.
// Note: TableID and PartitionID are left as uninitialized value.
// Go 直接调用 ddl.BuildTableInfoFromAST(metabuild.NewContext(), s)，这里保留该转发形状。
/// 由 CREATE TABLE AST 构建表元数据，供单测使用。
pub fn BuildTableInfoFromASTForTest(
    s: &ast::CreateTableStmt,
) -> Result<model::TableInfo, parser::errors::Error> {
    // 空依赖的 metabuild 上下文，与 Go NewContext 空切片等价。
    let context = metabuild::NewContext::<(), Infallible>(Vec::new());
    ddl::BuildTableInfoFromAST(&context, s)
}

// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Rust counterpart of `TestCreateMultiValuedIndexHasBinaryCollation`.
//
// Go drives `testkit`/`mockstore`, creates the table, then walks
// `InfoSchema.TableByName(...).Cols()` looking for the hidden MV-index
// column. Without a full SQL executor this crate builds the same table
// metadata through production `BuildTableInfoFromAST` and asserts the
// hidden array column uses binary charset/collation — the property Go
// checks after DDL.
//
// 校验创建多值索引时，隐藏的数组列使用 binary 字符集与排序规则
// （collation：决定字符串比较与排序的规则）。

use astersql_ddl::BuildTableInfoFromAST;
use astersql_meta_metabuild as metabuild;
use astersql_parser_ast as ast;

use crate::main_test::ensure_test_env;

/// 解析单条 `CREATE TABLE` SQL，并取回其拥有权以交给 metabuild。
fn parse_create(sql: &str) -> Box<ast::CreateTableStmt> {
    astersql_parser::New()
        .ParseOneStmt(sql, "", "")
        .expect("parse CREATE TABLE")
        .into_any()
        .downcast::<ast::CreateTableStmt>()
        .expect("CREATE TABLE AST")
}

// test_create_multi_valued_index_has_binary_collation 对应 Go 的
// TestCreateMultiValuedIndexHasBinaryCollation。
/// 创建含 CAST(... AS ... ARRAY) 的多值索引后，隐藏列须为 binary 数组类型。
#[test]
fn test_create_multi_valued_index_has_binary_collation() {
    ensure_test_env();

    // Go: create table test.t (pk varchar(4) primary key clustered, j json,
    //     str varchar(255), value int, key idx((cast(j as char(100) array)), str));
    let statement = parse_create(
        "create table test.t (pk varchar(4) primary key clustered, j json, \
         str varchar(255), value int, key idx((cast(j as char(100) array)), str))",
    );
    let context = metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let table = BuildTableInfoFromAST(&context, &statement).expect("build table metadata");

    // Go: is := tk.Session().GetLatestInfoSchema(); tbl, err := is.TableByName(...)
    assert_eq!(table.Name.O, "t");
    assert!(
        table.Indices.iter().any(|index| index.MVIndex),
        "multi-valued index should set MVIndex"
    );

    // 遍历列，找到 MV 索引生成的隐藏数组列并断言 charset/collation。
    let mut found_index = false;
    for column in &table.Columns {
        if column.Hidden {
            found_index = true;
            assert!(
                column.FieldType.IsArray(),
                "hidden MV-index column must be an array type"
            );
            assert_eq!(column.FieldType.GetCharset(), "binary");
            assert_eq!(column.FieldType.GetCollate(), "binary");
            assert!(
                column
                    .GeneratedExprString
                    .to_ascii_lowercase()
                    .contains("array"),
                "generated expression should retain array cast: {}",
                column.GeneratedExprString
            );
        }
    }
    assert!(found_index, "expected a hidden column for the MV index");
}

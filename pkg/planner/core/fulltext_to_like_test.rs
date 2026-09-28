// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 全文检索 modifier 与 public FTS 索引探测相关测试。
//
// 对齐 Go：`TestFTSModifierAllowsNativePushdown` 与
// `TestTableHasPublicFTSIndexOnColumn`，验证何种 MATCH modifier 允许原生下推，
// 以及表上指定列是否存在 StatePublic 的全文索引。

use super::expression_rewriter::{ftsModifierAllowsNativePushdown, tableHasPublicFTSIndexOnColumn};
use super::fulltext_to_like::convertMatchAgainstToLike;
use expression_dependency::{ast, model};
use exprstatic_dependency::NewExprContext;

// FtsModifierCase 对应 Go 表驱动用例：输入全文检索 modifier，并断言是否允许原生下推。
struct FtsModifierCase {
    name: &'static str,
    modifier: u8,
    expected: bool,
}

// test_fts_modifier_allows_native_pushdown 对应 Go 的 TestFTSModifierAllowsNativePushdown。
// Go 通过 t.Run 逐个执行表驱动用例；这里保留同样的用例顺序和断言语义。
#[test]
fn test_fts_modifier_allows_native_pushdown() {
    let tests = vec![
        FtsModifierCase {
            name: "natural language mode (default)",
            modifier: 0,
            expected: true,
        },
        FtsModifierCase {
            name: "boolean mode",
            modifier: 1,
            expected: false,
        },
        FtsModifierCase {
            name: "natural language mode with query expansion",
            modifier: 16,
            expected: false,
        },
    ];

    for tt in tests {
        assert_eq!(
            tt.expected,
            ftsModifierAllowsNativePushdown(tt.modifier),
            "case {}",
            tt.name
        );
    }
}

// Go 的 convertMatchAgainstToLike 只是 expression.BuildFTSToILikeExpression 的薄封装；
// 必须保留表达式类型、modifier 位布局以及底层错误传播，不能退化为 SQL 字符串拼接。
#[test]
fn test_convert_match_against_to_like_delegates_to_expression_builder() {
    let context = NewExprContext(Vec::new());

    let no_columns = convertMatchAgainstToLike(&context, Vec::new(), "alpha".to_owned(), 0)
        .err()
        .expect("MATCH without columns must be rejected");
    assert!(no_columns.to_string().contains("no columns"));

    let query_expansion = convertMatchAgainstToLike(
        &context,
        vec![Box::new(expression_dependency::NewStrConst("alpha"))],
        "alpha".to_owned(),
        1 << 4,
    )
    .err()
    .expect("WITH QUERY EXPANSION must be rejected");
    assert!(query_expansion.to_string().contains("WITH QUERY EXPANSION"));

    let result = convertMatchAgainstToLike(
        &context,
        vec![Box::new(expression_dependency::NewStrConst("alpha"))],
        String::new(),
        0,
    );
    assert!(result.is_ok(), "{:#?}", result.err());
}

// FtsIndexCase 对应 Go 第二个表驱动用例：构造表索引列表并检查指定列是否拥有 public FTS 索引。
struct FtsIndexCase {
    name: &'static str,
    indices: Vec<model::IndexInfo>,
    column: &'static str,
    expected: bool,
}

/// 构造带 FullTextInfo 的索引，便于控制 SchemaState。
fn fts_idx(name: &str, column: &str, state: model::SchemaState) -> model::IndexInfo {
    model::IndexInfo {
        Name: ast::NewCIStr(name),
        State: state,
        Columns: vec![model::IndexColumn {
            Name: ast::NewCIStr(column),
            ..Default::default()
        }],
        FullTextInfo: Some(model::FullTextIndexInfo {
            ParserType: model::FullTextParserTypeStandardV1.clone(),
        }),
        ..Default::default()
    }
}

// plain_idx 对应 Go 闭包 plainIdx，生成 public btree 普通索引用于反例。
fn plain_idx(name: &str, column: &str) -> model::IndexInfo {
    model::IndexInfo {
        Name: ast::NewCIStr(name),
        State: model::StatePublic,
        Columns: vec![model::IndexColumn {
            Name: ast::NewCIStr(column),
            ..Default::default()
        }],
        ..Default::default()
    }
}

// test_table_has_public_fts_index_on_column 对应 Go 的 TestTableHasPublicFTSIndexOnColumn。
// 重点覆盖无索引、普通索引、非 public FTS、不同列、多个索引和大小写不敏感匹配。
#[test]
fn test_table_has_public_fts_index_on_column() {
    let tests = vec![
        FtsIndexCase {
            name: "no indices",
            indices: vec![],
            column: "title",
            expected: false,
        },
        FtsIndexCase {
            name: "only non-FTS index on the column",
            indices: vec![plain_idx("idx_title", "title")],
            column: "title",
            expected: false,
        },
        FtsIndexCase {
            name: "public FTS index on the column",
            indices: vec![fts_idx("ft_title", "title", model::StatePublic)],
            column: "title",
            expected: true,
        },
        FtsIndexCase {
            name: "non-public FTS index on the column",
            indices: vec![fts_idx(
                "ft_title",
                "title",
                model::SchemaState::WriteReorganization,
            )],
            column: "title",
            expected: false,
        },
        FtsIndexCase {
            name: "FTS index on a different column",
            indices: vec![fts_idx("ft_body", "body", model::StatePublic)],
            column: "title",
            expected: false,
        },
        FtsIndexCase {
            name: "FTS index covers the column among many indices",
            indices: vec![
                plain_idx("idx_id", "id"),
                fts_idx("ft_body", "body", model::StatePublic),
                fts_idx("ft_title", "title", model::StatePublic),
            ],
            column: "title",
            expected: true,
        },
        FtsIndexCase {
            name: "case-insensitive column match",
            indices: vec![fts_idx("ft_title", "Title", model::StatePublic)],
            column: "title",
            expected: true,
        },
    ];

    for tt in tests {
        let tbl_info = model::TableInfo {
            Indices: tt.indices,
            ..Default::default()
        };
        assert_eq!(
            tt.expected,
            tableHasPublicFTSIndexOnColumn(&tbl_info, tt.column),
            "case {}",
            tt.name
        );
    }
}

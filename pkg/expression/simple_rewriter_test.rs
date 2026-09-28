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

use crate::*;
use std::sync::Arc;

fn field(db: &str, table: &str, column: &str, redundant: bool) -> Arc<types::FieldName> {
    Arc::new(types::FieldName {
        DBName: ast::NewCIStr(db),
        TblName: ast::NewCIStr(table),
        ColName: ast::NewCIStr(column),
        Redundant: redundant,
        ..Default::default()
    })
}

fn column(schema: &str, table: &str, name: &str) -> ast::ColumnName {
    ast::ColumnName {
        Schema: ast::NewCIStr(schema),
        Table: ast::NewCIStr(table),
        Name: ast::NewCIStr(name),
    }
}

fn names(fields: Vec<Arc<types::FieldName>>) -> types::NameSlice {
    types::NameSlice(fields.into_iter().map(Some).collect())
}

#[test]
fn find_field_name_matches_every_go_case() {
    struct Case {
        label: &'static str,
        names: types::NameSlice,
        column: ast::ColumnName,
        expected: Option<usize>,
        ambiguous: bool,
    }
    let f = field;
    let cases = vec![
        Case {
            label: "Simple match",
            names: names(vec![f("db", "tbl", "col", false)]),
            column: column("db", "tbl", "col"),
            expected: Some(0),
            ambiguous: false,
        },
        Case {
            label: "Match with empty schema and table",
            names: names(vec![f("db", "tbl", "col", false)]),
            column: column("", "", "col"),
            expected: Some(0),
            ambiguous: false,
        },
        Case {
            label: "Match with empty schema, non-empty table",
            names: names(vec![f("db", "tbl", "col", false)]),
            column: column("", "tbl", "col"),
            expected: Some(0),
            ambiguous: false,
        },
        Case {
            label: "Match with non-empty schema, empty table",
            names: names(vec![f("db", "tbl", "col", false)]),
            column: column("db", "", "col"),
            expected: Some(0),
            ambiguous: false,
        },
        Case {
            label: "No match",
            names: names(vec![f("db", "tbl", "col1", false)]),
            column: column("db", "tbl", "col2"),
            expected: None,
            ambiguous: false,
        },
        Case {
            label: "Match with redundant field",
            names: names(vec![
                f("db", "tbl", "col", true),
                f("db", "tbl", "col", true),
                f("db", "tbl", "col", false),
            ]),
            column: column("db", "tbl", "col"),
            expected: Some(2),
            ambiguous: false,
        },
        Case {
            label: "Non-unique match",
            names: names(vec![
                f("db", "tbl", "col", false),
                f("db", "tbl", "col", false),
            ]),
            column: column("db", "tbl", "col"),
            expected: None,
            ambiguous: true,
        },
        Case {
            label: "Match with empty schema and table and redundant",
            names: names(vec![
                f("db", "tbl", "col", true),
                f("db", "tbl", "col", false),
            ]),
            column: column("", "", "col"),
            expected: Some(1),
            ambiguous: false,
        },
        Case {
            label: "Non-unique match with a redundant",
            names: names(vec![
                f("db", "tbl", "col", true),
                f("db", "tbl", "col", false),
                f("db", "tbl", "col", false),
            ]),
            column: column("db", "tbl", "col"),
            expected: None,
            ambiguous: true,
        },
        Case {
            label: "Match with multiple redundant",
            names: names(vec![
                f("db", "tbl", "col", true),
                f("db", "tbl", "col", true),
                f("db", "tbl", "col", true),
            ]),
            column: column("db", "tbl", "col"),
            expected: Some(0),
            ambiguous: false,
        },
    ];
    for case in cases {
        let result = FindFieldName(&case.names, &case.column);
        if case.ambiguous {
            let error = result.expect_err(case.label).to_string();
            assert!(
                error.contains("db.tbl.col") && error.contains("ambiguous"),
                "{}: {error}",
                case.label
            );
        } else {
            assert_eq!(result.unwrap(), case.expected, "{}", case.label);
        }
    }
}

#[test]
fn find_field_name_skips_unusable_and_absent_rust_entries() {
    let mut unusable = (*field("db", "tbl", "col", false)).Clone();
    unusable.NotExplicitUsable = true;
    let fields = types::NameSlice(vec![
        None,
        Some(Arc::new(unusable)),
        Some(field("db", "tbl", "col", false)),
    ]);
    assert_eq!(
        FindFieldName(&fields, &column("db", "tbl", "col")).unwrap(),
        Some(2)
    );
}

#[test]
fn find_field_name_idx_by_column_name_returns_first_match() {
    let fields = vec![
        (*field("db", "tbl", "other", false)).Clone(),
        (*field("db", "tbl", "col", false)).Clone(),
        (*field("db2", "tbl2", "col", false)).Clone(),
    ];
    assert_eq!(FindFieldNameIdxByColName(&fields, "col"), Some(1));
    assert_eq!(FindFieldNameIdxByColName(&fields, "missing"), None);
}

#[test]
fn parse_simple_expr_rejects_empty_input_before_parsing() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let error = match ParseSimpleExpr(&ctx, "", Vec::new()) {
        Ok(_) => panic!("empty input must fail"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "expression should not be an empty string"
    );
}

struct CustomParserContext {
    base: exprstatic::ExprContext,
}

impl BuildContext for CustomParserContext {
    fn ParseSQL(
        &self,
        sql: &str,
    ) -> Option<Result<(Vec<Box<dyn ast::Node>>, Vec<parser::errors::Error>), parser::errors::Error>>
    {
        assert_eq!(sql, "select custom syntax");
        Some(Err(parser::errors::New("custom parser used")))
    }

    fn GetEvalCtx(&self) -> &dyn EvalContext {
        exprctx::BuildContext::GetEvalCtx(&self.base)
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        exprctx::BuildContext::GetCharsetInfo(&self.base)
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        exprctx::BuildContext::GetDefaultCollationForUTF8MB4(&self.base)
    }
    fn GetBlockEncryptionMode(&self) -> String {
        exprctx::BuildContext::GetBlockEncryptionMode(&self.base)
    }
    fn GetSysdateIsNow(&self) -> bool {
        exprctx::BuildContext::GetSysdateIsNow(&self.base)
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        exprctx::BuildContext::GetNoopFuncsMode(&self.base)
    }
    fn Rng(&self) -> &mathutil::MysqlRng {
        exprctx::BuildContext::Rng(&self.base)
    }
    fn IsUseCache(&self) -> bool {
        exprctx::BuildContext::IsUseCache(&self.base)
    }
    fn SetSkipPlanCache(&self, reason: &str) {
        exprctx::BuildContext::SetSkipPlanCache(&self.base, reason)
    }
    fn AllocPlanColumnID(&self) -> i64 {
        exprctx::BuildContext::AllocPlanColumnID(&self.base)
    }
    fn IsInNullRejectCheck(&self) -> bool {
        false
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }
    fn ConnectionID(&self) -> u64 {
        exprctx::BuildContext::ConnectionID(&self.base)
    }
    fn IsReadonlyUserVar(&self, name: &str) -> bool {
        exprctx::BuildContext::IsReadonlyUserVar(&self.base, name)
    }
}

#[test]
fn parse_simple_expr_prefers_context_parser_like_go() {
    let ctx = CustomParserContext {
        base: exprstatic::NewExprContext(Vec::new()),
    };
    let error = match ParseSimpleExpr(&ctx, "custom syntax", Vec::new()) {
        Ok(_) => panic!("the custom parser error must be returned"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "custom parser used");
}

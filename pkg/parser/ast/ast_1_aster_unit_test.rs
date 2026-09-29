// Copyright 2026 AsterSQL.
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

// AST 核心接口与文本转换的迁移核对测试。
//
// 对照 Go：标志位取值、`GetStmtLabel` 特例、节点原文/位置，以及不可打印字符串字面量转 `0x` 十六进制。
use crate::{ast::*, base::*};
use parser_charset::{CharsetGBK, CharsetUTF8, FindEncoding};

#[test]
fn go_merge_7_walk_in_place_preserves_order_mutation_and_control_flow() {
    struct Recorder {
        events: Vec<&'static str>,
        skip: bool,
        stop: bool,
    }
    impl crate::InPlaceVisitor for Recorder {
        fn enter(&mut self, node: &mut dyn crate::Node) -> bool {
            if let Some(expr) = node.as_any_mut().downcast_mut::<crate::ExprNode>() {
                expr.OriginTextPosition = 7;
                self.events.push("expr enter");
            } else {
                self.events.push("stmt enter");
            }
            self.skip
        }
        fn leave(&mut self, node: &mut dyn crate::Node) -> bool {
            self.events.push(if node.as_any().is::<crate::ExprNode>() {
                "expr leave"
            } else {
                "stmt leave"
            });
            !self.stop
        }
    }
    let mut stmt = crate::DoStmt {
        Exprs: vec![crate::ExprNode::default()],
        ..crate::DoStmt::default()
    };
    let mut visitor = Recorder {
        events: Vec::new(),
        skip: false,
        stop: false,
    };
    assert!(crate::Walk(&mut stmt, &mut visitor));
    assert_eq!(
        visitor.events,
        ["stmt enter", "expr enter", "expr leave", "stmt leave"]
    );
    assert_eq!(stmt.Exprs[0].OriginTextPosition, 7);
    let mut visitor = Recorder {
        events: Vec::new(),
        skip: true,
        stop: false,
    };
    assert!(crate::Walk(&mut stmt, &mut visitor));
    assert_eq!(visitor.events, ["stmt enter", "stmt leave"]);
    let mut visitor = Recorder {
        events: Vec::new(),
        skip: false,
        stop: true,
    };
    assert!(!crate::Walk(&mut stmt, &mut visitor));
    assert_eq!(visitor.events, ["stmt enter", "expr enter", "expr leave"]);
}

#[test]
fn go_merge_7_materialized_view_visits_table_name() {
    struct Rename;
    impl crate::InPlaceVisitor for Rename {
        fn enter(&mut self, _node: &mut dyn crate::Node) -> bool {
            false
        }
        fn leave(&mut self, _node: &mut dyn crate::Node) -> bool {
            true
        }
        fn enter_table_name(&mut self, table: &mut crate::TableName) -> bool {
            table.Name.O = "renamed".into();
            true
        }
    }
    let mut stmt = crate::DropMaterializedViewStmt {
        ViewName: Some(crate::TableName::default()),
        ..crate::DropMaterializedViewStmt::default()
    };
    assert!(crate::Walk(&mut stmt, &mut Rename));
    assert_eq!(stmt.ViewName.unwrap().Name.O, "renamed");
    let mut absent = crate::DropMaterializedViewStmt::default();
    assert!(crate::Walk(&mut absent, &mut Rename));
    assert!(absent.ViewName.is_none());
}

#[test]
fn go_merge_7_partition_in_place_stops_in_value_order() {
    struct StopAfterOne(usize);
    impl crate::InPlaceVisitor for StopAfterOne {
        fn enter(&mut self, node: &mut dyn crate::Node) -> bool {
            if node.as_any().is::<crate::ExprNode>() {
                self.0 += 1;
            }
            false
        }
        fn leave(&mut self, _node: &mut dyn crate::Node) -> bool {
            self.0 < 1
        }
    }
    let mut clause = crate::PartitionDefinitionClause::In(vec![
        vec![crate::ExprNode::default(), crate::ExprNode::default()],
        vec![crate::ExprNode::default()],
    ]);
    let mut visitor = StopAfterOne(0);
    assert!(!crate::walk::MutChildren::visit_children_mut(
        &mut clause,
        &mut visitor
    ));
    assert_eq!(visitor.0, 1);
}

#[test]
fn go_merge_7_materialized_view_statement_labels() {
    let cases = [
        (
            StatementKind::CreateMaterializedView,
            "CreateMaterializedView",
        ),
        (
            StatementKind::CreateMaterializedViewLog,
            "CreateMaterializedViewLog",
        ),
        (
            StatementKind::AlterMaterializedView,
            "AlterMaterializedView",
        ),
        (
            StatementKind::AlterMaterializedViewLog,
            "AlterMaterializedViewLog",
        ),
        (StatementKind::DropMaterializedView, "DropMaterializedView"),
        (
            StatementKind::DropMaterializedViewLog,
            "DropMaterializedViewLog",
        ),
        (
            StatementKind::PurgeMaterializedViewLog,
            "PurgeMaterializedViewLog",
        ),
        (
            StatementKind::RefreshMaterializedView,
            "RefreshMaterializedView",
        ),
        (
            StatementKind::CancelMaterializedViewJob,
            "CancelMaterializedViewJob",
        ),
    ];
    for (kind, label) in cases {
        assert_eq!(GetStmtLabel(&kind), label);
    }
}

#[test]
/// 核对表达式 Flag 常量与 GetStmtLabel 特例标签与 Go 一致。
fn ast_flags_and_statement_labels_match_go() {
    assert_eq!(FlagConstant, 0);
    assert_eq!(FlagHasParamMarker, 1 << 0);
    assert_eq!(FlagHasFunc, 1 << 1);
    assert_eq!(FlagHasReference, 1 << 2);
    assert_eq!(FlagHasAggregateFunc, 1 << 3);
    assert_eq!(FlagHasSubquery, 1 << 4);
    assert_eq!(FlagHasVariable, 1 << 5);
    assert_eq!(FlagHasDefault, 1 << 6);
    assert_eq!(FlagPreEvaluated, 1 << 7);
    assert_eq!(FlagHasWindowFunc, 1 << 8);

    let cases = [
        (StatementKind::AlterTable, "AlterTable"),
        (StatementKind::AnalyzeTable, "AnalyzeTable"),
        (StatementKind::Begin, "Begin"),
        (StatementKind::Commit, "Commit"),
        (StatementKind::CompactTable, "CompactTable"),
        (StatementKind::CreateDatabase, "CreateDatabase"),
        (StatementKind::CreateIndex, "CreateIndex"),
        (StatementKind::CreateTable, "CreateTable"),
        (StatementKind::CreateView, "CreateView"),
        (StatementKind::CreateUser, "CreateUser"),
        (StatementKind::Delete, "Delete"),
        (StatementKind::DropDatabase, "DropDatabase"),
        (StatementKind::DropIndex, "DropIndex"),
        (StatementKind::DropTable { is_view: false }, "DropTable"),
        (StatementKind::DropTable { is_view: true }, "DropView"),
        (
            StatementKind::Explain {
                show: true,
                analyze: false,
            },
            "DescTable",
        ),
        (
            StatementKind::Explain {
                show: true,
                analyze: true,
            },
            "DescTable",
        ),
        (
            StatementKind::Explain {
                show: false,
                analyze: true,
            },
            "ExplainAnalyzeSQL",
        ),
        (
            StatementKind::Explain {
                show: false,
                analyze: false,
            },
            "ExplainSQL",
        ),
        (StatementKind::Insert { is_replace: false }, "Insert"),
        (StatementKind::Insert { is_replace: true }, "Replace"),
        (StatementKind::ImportInto, "ImportInto"),
        (StatementKind::LoadData, "LoadData"),
        (StatementKind::Rollback, "Rollback"),
        (StatementKind::Select, "Select"),
        (StatementKind::Set, "Set"),
        (StatementKind::SetPassword, "Set"),
        (StatementKind::Show, "Show"),
        (StatementKind::TruncateTable, "TruncateTable"),
        (StatementKind::Update, "Update"),
        (StatementKind::Grant, "Grant"),
        (StatementKind::Revoke, "Revoke"),
        (StatementKind::Deallocate, "Deallocate"),
        (StatementKind::Execute, "Execute"),
        (StatementKind::Prepare, "Prepare"),
        (StatementKind::Use, "Use"),
        (StatementKind::CreateBinding, "CreateBinding"),
        (StatementKind::DropBinding, "DropBinding"),
        (StatementKind::Trace, "Trace"),
        (StatementKind::Shutdown, "Shutdown"),
        (StatementKind::Savepoint, "Savepoint"),
        (StatementKind::OptimizeTable, "Optimize"),
        (StatementKind::Other, "other"),
    ];
    for (statement, expected) in cases {
        assert_eq!(GetStmtLabel(&statement), expected);
    }
}

#[test]
/// 核对节点原文位置、UTF-8 文本及表达式类型/标志读写。
fn node_text_positions_and_expression_state_match_go() {
    let mut n = AstNode::default();
    n.SetOriginTextPosition(37);
    n.SetText(FindEncoding(CharsetUTF8), "你好".as_bytes());
    assert_eq!(n.OriginTextPosition(), 37);
    assert_eq!(n.Text(), "你好");
    assert_eq!(n.OriginalText(), "你好".as_bytes());

    let mut expr = ExprNodeBase::default();
    let mut field_type = FieldType::default();
    field_type.SetType(15);
    expr.SetType(field_type);
    expr.SetFlag(FlagHasFunc | FlagHasReference);
    assert_eq!(expr.GetType().GetType(), 15);
    assert_eq!(expr.GetFlag(), FlagHasFunc | FlagHasReference);
}

#[test]
/// 核对二进制/不可打印字面量转换，以及注释内引号不被误解析；含 GBK。
fn binary_literals_and_comments_match_go() {
    let cases: &[(&[u8], &str)] = &[
        (b"SELECT 'hello world'", "SELECT 'hello world'"),
        (b"SELECT '\xd2\xe4'", "SELECT 0xd2e4"),
        (b"SELECT _binary'\x01'", "SELECT _binary 0x01"),
        (b"SELECT '\xd2''\xe4'", "SELECT 0xd227e4"),
        (b"-- don't\nSELECT '\xd2\xe4'", "-- don't\nSELECT 0xd2e4"),
        (
            b"/* it's quoted */ SELECT 'value'",
            "/* it's quoted */ SELECT 'value'",
        ),
        (
            b"/*!80000 SELECT '\xd2\xe4' */",
            "/*!80000 SELECT 0xd2e4 */",
        ),
        (
            b"SELECT 'hello\x00world'",
            "SELECT 0x68656c6c6f00776f726c64",
        ),
    ];
    let mut n = AstNode::default();
    for (input, expected) in cases {
        n.SetText(FindEncoding(CharsetUTF8), input);
        assert_eq!(n.Text(), *expected, "input={input:?}");
    }

    n.SetText(FindEncoding(CharsetGBK), b"select '\xb1\xed1'");
    assert_eq!(n.Text(), "select '表1'");
    n.SetText(FindEncoding(CharsetGBK), b"select '\x80\xff'");
    assert_eq!(n.Text(), "select 0x80ff");
}

#[test]
/// 开启 NO_BACKSLASH_ESCAPES 后应失效缓存并按新规则重算 Text。
fn no_backslash_escapes_recomputes_cached_text() {
    let mut n = AstNode::default();
    n.SetText(FindEncoding(CharsetUTF8), b"SELECT '\\n'");
    assert_eq!(n.Text(), "SELECT '\\n'");
    n.SetNoBackslashEscapes(true);
    assert_eq!(n.Text(), "SELECT '\\n'");

    n.SetText(FindEncoding(CharsetUTF8), b"SELECT '\xd2\xe4'");
    assert_eq!(n.Text(), "SELECT 0xd2e4");
}

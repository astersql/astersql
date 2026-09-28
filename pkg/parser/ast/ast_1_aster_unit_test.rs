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

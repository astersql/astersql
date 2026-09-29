// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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
// AST 还原/只读判定相关的测试辅助与用例。
//
// 提供与 Go 对齐的节点文本清洗、Restore 往返比较流水线，以及
// `IsReadOnly` 在常见语句与 UNION 行锁组合上的断言。

use std::fmt::Debug;

use crate::util::IsReadOnly;
use crate::{
    AdminStmt, AdminStmtType, DeleteStmt, DoStmt, ExplainStmt, InPlaceVisitor, InsertStmt, Node,
    SelectLockType, SelectStmt, SetOprSelectList, SetOprStmt, ShowStmt, TraceStmt, UpdateStmt,
    VariableExpr, Walk,
};

/// Rust counterpart of Go's `nodeTextCleaner` contract.
///
/// Concrete parser nodes implement their Go-specific normalization here: clear source text and
/// origin positions, remove the underscore-charset flag, normalize function/aggregate names,
/// clear field offsets, normalize alter options, remove explicit join parentheses, and clean
/// binary-literal element metadata.
///
/// 对照 Go 的 `nodeTextCleaner`：在 AST 结构相等比较前，清除源文本、位置等
/// 与解析现场相关、但不影响语义结构的字段。
pub trait NodeTextCleaner {
    fn clean_node_text(&mut self);
}

/// CleanNodeText sets the text of a node and all child nodes empty before AST equality checks.
/// 在 AST 相等比较前清空节点文本（及实现方约定的其它现场字段）。
#[allow(non_snake_case)]
pub fn CleanNodeText<T: NodeTextCleaner + ?Sized>(node: &mut T) {
    node.clean_node_text();
}

/// 单条 Restore 测试用例：源 SQL 片段与期望还原结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(non_snake_case)]
pub struct NodeRestoreTestCase<'a> {
    /// 填入模板 `%s` 的源 SQL 片段。
    pub sourceSQL: &'a str,
    /// 填入模板 `%s` 的期望还原 SQL 片段。
    pub expectSQL: &'a str,
}

/// Restore 行为标志位集合（与 format 包标志语义对齐）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestoreFlags(pub u64);

/// 默认还原标志（无额外选项）。
#[allow(non_upper_case_globals)]
pub const DefaultRestoreFlags: RestoreFlags = RestoreFlags(0);

/// 将 `value` 替换模板中首个 `%s`，模拟 Go 测试里的 SQL 模板拼装。
fn apply_template(template: &str, value: &str) -> String {
    template.replacen("%s", value, 1)
}

/// Go-compatible Restore test pipeline: parse source, restore the selected node, parse the
/// restored SQL again, normalize both ASTs, and compare their complete structures.
///
/// 与 Go 对齐的 Restore 测试流水线：解析 → 还原 → 再解析 → 清洗 → 结构比较。
#[allow(non_snake_case)]
pub fn runNodeRestoreTest<T, Parse, Restore>(
    node_test_cases: &[NodeRestoreTestCase<'_>],
    template: &str,
    parse: Parse,
    restore: Restore,
) where
    T: Clone + Debug + PartialEq + NodeTextCleaner,
    Parse: Fn(&str) -> Result<T, String>,
    Restore: Fn(&T, RestoreFlags) -> Result<String, String>,
{
    runNodeRestoreTestWithFlags(
        node_test_cases,
        template,
        parse,
        restore,
        DefaultRestoreFlags,
    );
}

/// 带自定义 RestoreFlags 的完整往返比较流水线。
#[allow(non_snake_case)]
pub fn runNodeRestoreTestWithFlags<T, Parse, Restore>(
    node_test_cases: &[NodeRestoreTestCase<'_>],
    template: &str,
    parse: Parse,
    restore: Restore,
    flags: RestoreFlags,
) where
    T: Clone + Debug + PartialEq + NodeTextCleaner,
    Parse: Fn(&str) -> Result<T, String>,
    Restore: Fn(&T, RestoreFlags) -> Result<String, String>,
{
    for test_case in node_test_cases {
        let source_sql = apply_template(template, test_case.sourceSQL);
        let expect_sql = apply_template(template, test_case.expectSQL);
        let mut statement =
            parse(&source_sql).unwrap_or_else(|error| panic!("source {test_case:?}: {error}"));
        let restored_node = restore(&statement, flags)
            .unwrap_or_else(|error| panic!("source {test_case:?}: {error}"));
        let restored_sql = apply_template(template, &restored_node);
        assert_eq!(restored_sql, expect_sql, "source {test_case:?}");

        // 再解析还原结果，清洗现场字段后比较完整 AST 结构。
        let mut reparsed = parse(&restored_sql).unwrap_or_else(|error| {
            panic!("source {test_case:?}; restore {restored_sql}: {error}")
        });
        CleanNodeText(&mut statement);
        CleanNodeText(&mut reparsed);
        assert_eq!(
            statement, reparsed,
            "source {test_case:?}; restore {restored_sql}"
        );
    }
}

/// Variant for Go cases where Restore intentionally changes the AST representation.
/// 变体：Restore 会有意改变 AST 形态时，只比较还原 SQL 文本，不做结构相等。
#[allow(non_snake_case)]
pub fn runNodeRestoreTestWithFlagsStmtChange<T, Parse, Restore>(
    node_test_cases: &[NodeRestoreTestCase<'_>],
    template: &str,
    parse: Parse,
    restore: Restore,
    flags: RestoreFlags,
) where
    T: Debug,
    Parse: Fn(&str) -> Result<T, String>,
    Restore: Fn(&T, RestoreFlags) -> Result<String, String>,
{
    for test_case in node_test_cases {
        let source_sql = apply_template(template, test_case.sourceSQL);
        let expect_sql = apply_template(template, test_case.expectSQL);
        let statement =
            parse(&source_sql).unwrap_or_else(|error| panic!("source {test_case:?}: {error}"));
        let restored_node = restore(&statement, flags)
            .unwrap_or_else(|error| panic!("source {test_case:?}: {error}"));
        assert_eq!(
            apply_template(template, &restored_node),
            expect_sql,
            "source {test_case:?}"
        );
    }
}

/// 本地夹具：模拟带源文本/位置元数据的可还原节点。
#[derive(Clone, Debug, Eq, PartialEq)]
struct RestoreFixture {
    canonical_sql: String,
    source_text: String,
    origin_position: usize,
}

impl NodeTextCleaner for RestoreFixture {
    fn clean_node_text(&mut self) {
        self.source_text.clear();
        self.origin_position = 0;
    }
}

/// 校验常见语句的只读性，并顺带演练 Restore 往返流水线。
#[test]
fn test_cacheable() {
    assert!(!IsReadOnly(&DeleteStmt::default(), true));
    assert!(!IsReadOnly(&InsertStmt::default(), true));
    assert!(!IsReadOnly(&UpdateStmt::default(), true));
    assert!(IsReadOnly(&DoStmt::default(), true));
    assert!(IsReadOnly(&ShowStmt::default(), true));
    assert!(IsReadOnly(
        &ExplainStmt::new(false, Box::new(InsertStmt::default())),
        true
    ));
    assert!(!IsReadOnly(
        &ExplainStmt::new(true, Box::new(InsertStmt::default())),
        true
    ));
    assert!(IsReadOnly(
        &ExplainStmt::new(false, Box::new(SelectStmt::default())),
        true
    ));
    assert!(IsReadOnly(
        &ExplainStmt::new(true, Box::new(SelectStmt::default())),
        true
    ));
    assert!(IsReadOnly(
        &TraceStmt::new(Box::new(SelectStmt::default())),
        true
    ));
    assert!(!IsReadOnly(
        &TraceStmt::new(Box::new(DeleteStmt::default())),
        true
    ));

    // Exercise the shared Go-compatible parse/restore/reparse/clean/compare pipeline.
    runNodeRestoreTest(
        &[NodeRestoreTestCase {
            sourceSQL: "select 1",
            expectSQL: "SELECT 1",
        }],
        "%s",
        |sql| {
            Ok(RestoreFixture {
                canonical_sql: sql.to_ascii_uppercase(),
                source_text: sql.into(),
                origin_position: 7,
            })
        },
        |node, _| Ok(node.canonical_sql.clone()),
    );
}

/// 按各分支可选行锁构造 UNION 形态的集合运算语句。
fn union(locks: &[Option<SelectLockType>]) -> SetOprStmt {
    SetOprStmt::new(SetOprSelectList::new(
        locks
            .iter()
            .map(|lock| match lock {
                Some(lock) => Box::new(SelectStmt::with_lock(*lock)) as Box<dyn Node>,
                None => Box::new(SelectStmt::default()) as Box<dyn Node>,
            })
            .collect(),
    ))
}

/// UNION 任一分支带 FOR UPDATE 类锁时整体非只读。
#[test]
fn test_union_read_only() {
    use SelectLockType::{ForUpdate, ForUpdateNoWait};
    assert!(IsReadOnly(&union(&[None, None]), true));
    assert!(IsReadOnly(&union(&[None, None, None]), true));
    for locks in [
        &[None, Some(ForUpdate)][..],
        &[None, Some(ForUpdateNoWait)],
        &[Some(ForUpdate), Some(ForUpdateNoWait)],
        &[None, Some(ForUpdate), Some(ForUpdateNoWait)],
    ] {
        assert!(!IsReadOnly(&union(locks), true));
    }
}

#[test]
fn test_read_only_branch_parity() {
    assert_eq!(crate::util::UNSPECIFIED_SIZE, u64::MAX);
    assert_eq!(crate::util::UnspecifiedSize, u64::MAX);

    for lock in [
        SelectLockType::ForUpdate,
        SelectLockType::ForUpdateNoWait,
        SelectLockType::ForUpdateWaitN,
        SelectLockType::ForShare,
        SelectLockType::ForShareNoWait,
    ] {
        assert!(!IsReadOnly(&SelectStmt::with_lock(lock), true));
    }
    assert!(IsReadOnly(
        &SelectStmt::with_lock(SelectLockType::ForUpdateSkipLocked),
        true
    ));

    let global_assignment = SelectStmt::with_child(Box::new(VariableExpr::new(true, true)));
    assert!(!IsReadOnly(&global_assignment, true));
    assert!(IsReadOnly(&global_assignment, false));
    assert!(IsReadOnly(
        &SelectStmt::with_child(Box::new(VariableExpr::new(false, true))),
        true
    ));
    assert!(IsReadOnly(
        &SelectStmt::with_child(Box::new(VariableExpr::new(true, false))),
        true
    ));

    let read_only_admin_types = [
        AdminStmtType::ShowDdl,
        AdminStmtType::ShowDdlJobs,
        AdminStmtType::ShowSlow,
        AdminStmtType::CaptureBindings,
        AdminStmtType::ShowNextRowId,
        AdminStmtType::ShowDdlJobQueries,
        AdminStmtType::ShowDdlJobQueriesWithRange,
    ];
    for statement_type in read_only_admin_types {
        assert!(IsReadOnly(&AdminStmt::new(statement_type), true));
    }
    assert!(!IsReadOnly(
        &AdminStmt::new(AdminStmtType::CheckTable),
        true
    ));

    assert!(IsReadOnly(&SetOprSelectList::new(Vec::new()), true));
    assert!(!IsReadOnly(
        &SetOprSelectList::new(vec![
            Box::new(SelectStmt::default()),
            Box::new(DeleteStmt::default()),
        ]),
        true
    ));
}

#[test]
fn go_merge_19_in_place_walk_preserves_read_only_variable_detection() {
    struct AssignmentChecker {
        found: bool,
    }

    impl InPlaceVisitor for AssignmentChecker {
        fn enter(&mut self, node: &mut dyn Node) -> bool {
            if let Some(variable) = node.as_any().downcast_ref::<VariableExpr>() {
                if variable.is_system && variable.value.is_some() {
                    self.found = true;
                    return true;
                }
            }
            false
        }

        fn leave(&mut self, _node: &mut dyn Node) -> bool {
            !self.found
        }
    }

    for (is_system, has_value, expected_read_only) in [
        (true, true, false),
        (true, false, true),
        (false, true, true),
    ] {
        let mut statement =
            SelectStmt::with_child(Box::new(VariableExpr::new(is_system, has_value)));
        let mut checker = AssignmentChecker { found: false };
        assert_eq!(Walk(&mut statement, &mut checker), expected_read_only);
        assert_eq!(IsReadOnly(&statement, true), expected_read_only);
        assert!(IsReadOnly(&statement, false));
    }
}

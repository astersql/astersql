// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// `hint_processor` 单测：HintsSet、还原、收集/绑定、查询块与 binding 完整性。
//
// 验证语句级 hint 过滤、索引 hint 文案、AST 遍历顺序、QB_NAME 去重告警、
// 视图 hint 拆分、GetCurrentStmtHints 与 ParseHintsSet，以及多表/TiFlash binding 边界。

use std::cell::RefCell;
use std::rc::Rc;

use super::*;

/// 构造 CIStr。
fn ci(value: &str) -> ast::CIStr {
    ast::NewCIStr(value)
}

/// 构造仅含 HintName 的 TableOptimizerHint。
fn hint(name: &str) -> ast::TableOptimizerHint {
    ast::TableOptimizerHint {
        HintName: ci(name),
        ..Default::default()
    }
}

/// 构造仅含表名的 HintTable。
fn table(name: &str) -> ast::HintTable {
    ast::HintTable {
        TableName: ci(name),
        ..Default::default()
    }
}

/// 构造带可选 IndexHints 的 TableSource ResultSet。
fn table_source(name: &str, index_hints: Vec<ast::IndexHint>) -> ast::ResultSetNode {
    ast::ResultSetNode::TableSource(ast::TableSource {
        Source: ast::TableName {
            Name: ci(name),
            IndexHints: index_hints,
            ..Default::default()
        },
        QuerySource: None,
        AsName: ast::CIStr::default(),
        TableSample: None,
        AsOf: None,
        Lateral: false,
        ColumnNames: Vec::new(),
    })
}

/// 将 ResultSet 包进单边 Join 的 TableRefsClause。
fn table_refs(result: ast::ResultSetNode) -> ast::TableRefsClause {
    ast::TableRefsClause {
        TableRefs: ast::Join {
            Left: Some(Box::new(result)),
            ..Default::default()
        },
    }
}

/// 共享可变警告列表的测试桩（便于跨断言读取）。
#[derive(Clone, Default)]
struct WarningSink(Rc<RefCell<Vec<String>>>);

impl hintWarnHandler for WarningSink {
    fn SetHintWarning(&mut self, warn: String) {
        self.0.borrow_mut().push(warn);
    }

    fn SetHintWarningFromError(&mut self, err: &dyn std::error::Error) {
        self.0.borrow_mut().push(err.to_string());
    }
}

/// GetStmtHints：首块全量 + 后续块仅语句级 hint；ContainTableHint 大小写敏感（O 字段）。
#[test]
fn hints_set_statement_filter_and_containment_match_go() {
    let mut join = hint("HASH_JOIN");
    join.Tables.push(table("t"));
    let quota = ast::TableOptimizerHint {
        HintName: ci("MEMORY_QUOTA"),
        HintData: ast::HintData::Signed(64 * 1024 * 1024),
        ..Default::default()
    };
    let max_time = ast::TableOptimizerHint {
        HintName: ci("MAX_EXECUTION_TIME"),
        HintData: ast::HintData::Unsigned(1000),
        ..Default::default()
    };
    let hs = HintsSet {
        tableHints: vec![
            vec![join.clone()],
            vec![quota.clone(), max_time.clone(), join],
        ],
        indexHints: Vec::new(),
    };

    assert_eq!(
        hs.GetStmtHints(),
        vec![hs.tableHints[0][0].clone(), quota, max_time]
    );
    assert!(hs.ContainTableHint("HASH_JOIN"));
    assert!(!hs.ContainTableHint("hash_join"));
}

/// RestoreOptimizerHints 去重；RestoreIndexHint 覆盖 FORCE/ORDER/NO ORDER 文案。
#[test]
fn restore_hints_and_index_hints_match_go_and_deduplicate() {
    let quota = ast::TableOptimizerHint {
        HintName: ci("MEMORY_QUOTA"),
        HintData: ast::HintData::Signed(64 * 1024 * 1024),
        ..Default::default()
    };
    assert_eq!(
        RestoreOptimizerHints(vec![quota.clone(), quota]),
        "memory_quota(64 mb)"
    );

    let index = ast::IndexHint {
        IndexNames: vec![ci("idx_a"), ci("idx_b")],
        HintType: ast::IndexHintType::Force,
        HintScope: ast::IndexHintScope::OrderBy,
    };
    assert_eq!(
        RestoreIndexHint(&index).unwrap(),
        "force index for order by (`idx_a`, `idx_b`)"
    );

    let mut ordering = index.clone();
    ordering.HintType = ast::IndexHintType::OrderIndex;
    ordering.HintScope = ast::IndexHintScope::Scan;
    assert_eq!(
        RestoreIndexHint(&ordering).unwrap(),
        "order index (`idx_a`, `idx_b`)"
    );
    ordering.HintType = ast::IndexHintType::NoOrderIndex;
    assert_eq!(
        RestoreIndexHint(&ordering).unwrap(),
        "no order index (`idx_a`, `idx_b`)"
    );
}

/// CollectHint / BindHint 遍历顺序与 Go 一致，能把 hint 写回空 AST。
#[test]
fn collect_and_bind_preserve_go_traversal_order() {
    let table_hint = hint("HASH_JOIN");
    let index_hint = ast::IndexHint {
        IndexNames: vec![ci("idx_a")],
        ..Default::default()
    };
    let stmt: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        TableHints: vec![table_hint.clone()],
        From: Some(table_refs(table_source("t", vec![index_hint.clone()]))),
        ..Default::default()
    });

    let collected = CollectHint(stmt.as_ref());
    assert_eq!(collected.tableHints, vec![vec![table_hint]]);
    assert_eq!(collected.indexHints, vec![vec![index_hint.clone()]]);

    let empty_stmt: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        From: Some(table_refs(table_source("t", Vec::new()))),
        ..Default::default()
    });
    let rebound = BindHint(empty_stmt, collected);
    let rebound = rebound.into_any().downcast::<ast::SelectStmt>().unwrap();
    assert_eq!(rebound.TableHints.len(), 1);
    let ast::ResultSetNode::TableSource(source) = rebound
        .From
        .as_ref()
        .unwrap()
        .TableRefs
        .Left
        .as_deref()
        .unwrap()
    else {
        panic!("expected table source");
    };
    assert_eq!(source.Source.IndexHints, vec![index_hint]);
}

/// Go 的统一 AST visitor 会进入 UPDATE/DELETE/SELECT 表达式中的子查询。
#[test]
fn expression_subqueries_are_collected_bound_and_numbered_in_go_order() {
    let outer_hint = hint("MEMORY_QUOTA");
    let mut inner_qb_name = hint("QB_NAME");
    inner_qb_name.QBName = ci("inner");
    let inner_hint = hint("HASH_JOIN");
    let inner: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        TableHints: vec![inner_qb_name.clone(), inner_hint.clone()],
        ..Default::default()
    });
    let where_expr = ast::ExprNode {
        node_text: Default::default(),
        Kind: ast::ExprKind::ExistsSubquery {
            Sel: Box::new(ast::ExprNode {
                node_text: Default::default(),
                Kind: ast::ExprKind::Subquery {
                    Query: ast::NodeRef::new(inner),
                    MultiRows: false,
                    Exists: true,
                },
                OriginTextPosition: 0,
                Flag: Default::default(),
            }),
            Not: false,
        },
        OriginTextPosition: 0,
        Flag: Default::default(),
    };
    let statement: Box<dyn ast::Node> = Box::new(ast::UpdateStmt {
        TableHints: vec![outer_hint.clone()],
        Where: Some(where_expr),
        ..Default::default()
    });

    let collected = CollectHint(statement.as_ref());
    assert_eq!(
        collected.tableHints,
        vec![
            vec![outer_hint.clone()],
            vec![inner_qb_name.clone(), inner_hint.clone()]
        ]
    );

    let empty_inner: Box<dyn ast::Node> = Box::new(ast::SelectStmt::default());
    let empty_where = ast::ExprNode {
        node_text: Default::default(),
        Kind: ast::ExprKind::Subquery {
            Query: ast::NodeRef::new(empty_inner),
            MultiRows: false,
            Exists: false,
        },
        OriginTextPosition: 0,
        Flag: Default::default(),
    };
    let rebound = BindHint(
        Box::new(ast::UpdateStmt {
            Where: Some(empty_where),
            ..Default::default()
        }),
        collected,
    );
    let rebound = rebound.into_any().downcast::<ast::UpdateStmt>().unwrap();
    assert_eq!(rebound.TableHints, vec![outer_hint]);
    let ast::ExprKind::Subquery { Query, .. } = &rebound.Where.as_ref().unwrap().Kind else {
        panic!("expected subquery expression");
    };
    Query
        .with_node(|node| {
            let inner = node.as_any().downcast_ref::<ast::SelectStmt>().unwrap();
            assert_eq!(inner.TableHints, vec![inner_qb_name.clone(), inner_hint]);
        })
        .unwrap();

    let mut handler = NewQBHintHandler(None);
    let processed = handler.Process(statement);
    assert_eq!(handler.MaxSelectStmtOffset(), 1);
    assert_eq!(handler.QBNameToSelOffset.get("inner"), Some(&1));
    let processed = processed.into_any().downcast::<ast::UpdateStmt>().unwrap();
    let ast::ExprKind::ExistsSubquery { Sel, .. } = &processed.Where.as_ref().unwrap().Kind else {
        panic!("expected exists subquery");
    };
    let ast::ExprKind::Subquery { Query, .. } = &Sel.Kind else {
        panic!("expected subquery expression");
    };
    Query
        .with_node(|node| {
            let inner = node.as_any().downcast_ref::<ast::SelectStmt>().unwrap();
            assert_eq!(inner.QueryBlockOffset, 1);
        })
        .unwrap();
}

/// Go 的派生表 Source 是查询节点，不会额外访问一个虚构的 TableName。
#[test]
fn derived_tables_do_not_shift_index_hint_binding_with_a_dummy_table() {
    let index_hint = ast::IndexHint {
        IndexNames: vec![ci("idx_inner")],
        ..Default::default()
    };
    let inner: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        From: Some(table_refs(table_source(
            "inner_t",
            vec![index_hint.clone()],
        ))),
        ..Default::default()
    });
    let derived = ast::ResultSetNode::TableSource(ast::TableSource {
        QuerySource: Some(ast::NodeRef::new(inner)),
        AsName: ci("derived_t"),
        ..Default::default()
    });
    let statement: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        From: Some(table_refs(derived)),
        ..Default::default()
    });

    let collected = CollectHint(statement.as_ref());
    assert_eq!(collected.tableHints, vec![vec![], vec![]]);
    assert_eq!(collected.indexHints, vec![vec![index_hint.clone()]]);

    let empty_inner: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        From: Some(table_refs(table_source("inner_t", Vec::new()))),
        ..Default::default()
    });
    let empty_derived = ast::ResultSetNode::TableSource(ast::TableSource {
        QuerySource: Some(ast::NodeRef::new(empty_inner)),
        AsName: ci("derived_t"),
        ..Default::default()
    });
    let rebound = BindHint(
        Box::new(ast::SelectStmt {
            From: Some(table_refs(empty_derived)),
            ..Default::default()
        }),
        collected,
    );
    let rebound = rebound.into_any().downcast::<ast::SelectStmt>().unwrap();
    let derived_result = rebound.From.unwrap().TableRefs.Left.unwrap();
    let ast::ResultSetNode::TableSource(derived) = derived_result.as_ref() else {
        panic!("expected derived table source");
    };
    derived
        .QuerySource
        .as_ref()
        .unwrap()
        .with_node(|node| {
            let inner = node.as_any().downcast_ref::<ast::SelectStmt>().unwrap();
            let ast::ResultSetNode::TableSource(source) = inner
                .From
                .as_ref()
                .unwrap()
                .TableRefs
                .Left
                .as_deref()
                .unwrap()
            else {
                panic!("expected inner table source");
            };
            assert_eq!(source.Source.IndexHints, vec![index_hint]);
        })
        .unwrap();
}

/// SetOprStmt.Accept 先访问自身 WITH，再访问 SelectList.WITH 和各 SELECT。
#[test]
fn set_operation_ctes_follow_go_accept_order() {
    let with_query = |name: &str| ast::WithClause {
        CTEs: vec![ast::CommonTableExpression {
            Name: ci(name),
            ColNameList: Vec::new(),
            Query: Box::new(ast::SelectStmt {
                TableHints: vec![hint(name)],
                ..Default::default()
            }),
            IsRecursive: false,
        }],
        ..Default::default()
    };
    let mut select_list = ast::SetOprSelectList::new(vec![Box::new(ast::SelectStmt {
        TableHints: vec![hint("branch")],
        ..Default::default()
    })]);
    select_list.With = Some(with_query("list_cte").into_shared());
    let statement = ast::SetOprStmt {
        node_text: Default::default(),
        select_list,
        OrderBy: Vec::new(),
        Limit: None,
        With: Some(with_query("statement_cte").into_shared()),
        IsInBraces: false,
    };

    let collected = CollectHint(&statement);
    assert_eq!(
        collected.tableHints,
        vec![
            vec![hint("statement_cte")],
            vec![hint("list_cte")],
            vec![hint("branch")]
        ]
    );
}

/// checkQueryBlockHints：同块多名取第一个；跨块重名告警 Duplicate。
#[test]
fn query_block_names_keep_first_mapping_and_report_duplicates() {
    let sink = WarningSink::default();
    let warnings = sink.0.clone();
    let mut handler = NewQBHintHandler(Some(Box::new(sink)));
    let mut first = hint("QB_NAME");
    first.QBName = ci("main");
    let mut second = hint("QB_NAME");
    second.QBName = ci("other");
    handler.checkQueryBlockHints(&[first.clone(), second], 1);
    handler.checkQueryBlockHints(&[first], 2);

    assert_eq!(handler.QBNameToSelOffset.get("main"), Some(&1));
    assert_eq!(warnings.borrow().len(), 2);
    assert!(warnings.borrow()[0].contains("using the first one main"));
    assert!(warnings.borrow()[1].contains("Duplicate query block name main"));
}

/// handleViewHints：视图 qb_name 与归属 hint 拆出；缺省表 QBName 填 sel_<offset>。
#[test]
fn view_hints_are_split_and_default_to_defining_block() {
    let mut handler = NewQBHintHandler(None);
    let mut view_name = hint("QB_NAME");
    view_name.QBName = ci("v");
    view_name.Tables = vec![table("view_t")];

    let mut view_hint = hint("HASH_JOIN");
    view_hint.QBName = ci("v");
    view_hint.Tables = vec![table("t1"), table("t2")];
    let normal = hint("MEMORY_QUOTA");

    let left = handler.handleViewHints(vec![view_name, view_hint.clone(), normal.clone()], 2);
    assert_eq!(left, vec![normal]);
    assert_eq!(handler.ViewQBNameToHints.get("v"), Some(&vec![view_hint]));
    assert_eq!(handler.ViewQBNameToTable["v"][0].QBName.L, "sel_2");
}

/// GetCurrentStmtHints 校验 offset 并复用 build state；GenerateQBName 边界。
#[test]
fn current_statement_hints_validate_offsets_and_reuse_build_state() {
    let sink = WarningSink::default();
    let warnings = sink.0.clone();
    let mut handler = NewQBHintHandler(Some(Box::new(sink)));
    handler.selectStmtOffset = 2;

    let mut top = hint("HASH_JOIN");
    top.QBName = ci("sel_1");
    let mut nested = hint("MERGE_JOIN");
    nested.QBName = ci("sel_2");
    let mut invalid = hint("USE_INDEX");
    invalid.QBName = ci("missing");
    invalid.Tables.push(table("t"));

    let mut state = QBHintBuildState::default();
    let current =
        handler.GetCurrentStmtHints(&[top.clone(), nested.clone(), invalid], 1, Some(&mut state));
    assert_eq!(current, vec![top]);
    assert_eq!(state.QBOffsetToHints.get(&2), Some(&vec![nested]));
    assert_eq!(warnings.borrow().len(), 1);
    assert_eq!(GenerateQBName(NodeType::TypeUpdate, 0).unwrap().L, "upd_1");
    assert_eq!(GenerateQBName(NodeType::TypeSelect, 2).unwrap().L, "sel_2");
    assert!(GenerateQBName(NodeType::TypeSelect, 0).is_err());
}

#[test]
fn extract_hint_warns_does_not_match_plain_error_text() {
    let warning = parser::errors::New("ordinary memory quota diagnostic");
    assert!(
        extractHintWarns(vec![warning]).is_empty(),
        "Go filters by exact parser error identity, not message substrings"
    );
}

#[test]
fn parse_hints_set_keeps_exact_parser_hint_warning() {
    let mut parser = NewParser();
    let (_, _, warns) = ParseHintsSet(
        &mut parser,
        "select /*+ unknown_hint() */ 1",
        "utf8mb4",
        "utf8mb4_bin",
        "test",
    )
    .unwrap();

    assert_eq!(warns.len(), 1);
}

/// ParseHintsSet 走真实 parser，还原语句级 memory_quota hint。
#[test]
fn parse_hints_set_uses_the_real_parser_dependency() {
    let mut parser = NewParser();
    let (hints, statement, warns) = ParseHintsSet(
        &mut parser,
        "select /*+ memory_quota(64 MB) */ 1",
        "utf8mb4",
        "utf8mb4_bin",
        "test",
    )
    .unwrap();

    assert_eq!(nodeType4Stmt(statement.as_ref()), NodeType::TypeSelect);
    assert!(warns.is_empty());
    assert_eq!(
        RestoreOptimizerHints(hints.GetStmtHints()),
        "memory_quota(64 mb)"
    );
}

/// CheckBindingFromHistoryComplete：>3 表 join 与含 tiflash 的 hint 均判定不完整。
#[test]
fn binding_history_completeness_matches_go_boundaries() {
    let left = ast::ResultSetNode::Join(Box::new(ast::Join {
        Left: Some(Box::new(table_source("a", Vec::new()))),
        Right: Some(Box::new(table_source("b", Vec::new()))),
        ..Default::default()
    }));
    let three_tables: Box<dyn ast::Node> = Box::new(ast::SelectStmt {
        From: Some(ast::TableRefsClause {
            TableRefs: ast::Join {
                Left: Some(Box::new(left)),
                Right: Some(Box::new(table_source("c", Vec::new()))),
                ..Default::default()
            },
        }),
        ..Default::default()
    });
    let (complete, reason) = CheckBindingFromHistoryComplete(three_tables.as_ref(), "");
    assert!(!complete);
    assert!(reason.contains("more than 3 table join"));

    let (complete, reason) =
        CheckBindingFromHistoryComplete(three_tables.as_ref(), "read_from_storage(tiflash[t])");
    assert!(!complete);
    assert!(reason.contains("TiFlash"));
}

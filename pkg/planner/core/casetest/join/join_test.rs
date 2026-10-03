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

// JOIN casetest：完整对照 Go `join_test.go` 的五个 `RunTestUnderCascades`
// 用例，在两种 planner 模式下真实建表、执行 SQL，并比较结果、plan_tree、
// warning 和索引选择。Hint/parser/sysvar 直连测试作为更精确的补充定位，
// 不替代端到端契约。
//
// 当前失败的真实 parity 断言指向 `pkg/session/runtime/explain_query.rs`
// 的简化 JOIN 计划渲染，以及 `pkg/session/runtime/dispatch.rs` 对 INTERSECT
// EXPLAIN 的 SELECT-only 限制；这些生产修复不属于本文件的修改范围。

#![allow(non_snake_case, non_upper_case_globals)]

use astersql_parser::Parser;
use astersql_parser::ast;
use astersql_util_hint::{
    ExtractTableHintsFromStmtNode, HintHashJoinBuild, HintINLHJ, NewQBHintHandler, ParsePlanHints,
    QBHintHandler, TiDBIndexNestedLoopJoin, hintWarnHandler,
};

/// 直接复现 Go `TestSemiJoinOrder` 的结果契约，防止 parser/hint 代理断言掩盖
/// JOIN 执行与物理计划链路缺失。
#[test]
fn test_semi_join_order_runs_go_result_contract() {
    for cascades in [false, true] {
        let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
        let mut tk = astersql_testkit::TestKit::new(store);
        tk.MustExec("use test", Vec::new());
        tk.MustExec("create table t1 (col0 int, col1 int)", Vec::new());
        tk.MustExec("create table t2 (col0 int, col1 int)", Vec::new());
        tk.MustExec(
            "insert into t1 values (null, 3), (null, 5), (null, null), (1, 1), \
             (1, 2), (1, null), (2, 1), (2, 2), (2, null), (3, 1), (3, 2), \
             (3, 4), (3, null)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t2 values (null, 3), (null, 4), (null, null), (1, 1), \
             (3, 1), (3, 3), (3, null), (4, null), (4, 1), (4, 2), (4, 10)",
            Vec::new(),
        );
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec("set tidb_hash_join_version=optimized", Vec::new());

        let expected =
            astersql_testkit::Rows(&["1 <nil>", "1 1", "1 2", "3 <nil>", "3 1", "3 2", "3 4"]);
        for sql in [
            "select * from t1 where exists (select 1 from t2 where t1.col0 = t2.col0) \
             order by t1.col0, t1.col1",
            "select /*+ HASH_JOIN_BUILD(t1) */ * from t1 where exists \
             (select 1 from t2 where t1.col0 = t2.col0) order by t1.col0, t1.col1",
            "select /*+ HASH_JOIN_BUILD(t2@sel_2) */ * from t1 where exists \
             (select 1 from t2 where t1.col0 = t2.col0) order by t1.col0, t1.col1",
        ] {
            tk.MustQuery(sql, Vec::new()).Check(expected.clone());
        }

        let optimized_t1_plan = astersql_testkit::Rows(&[
            "Sort root  test.t1.col0, test.t1.col1",
            "└─HashJoin root  semi join, left side:TableReader, equal:[eq(test.t1.col0, test.t2.col0)]",
            "  ├─TableReader(Build) root  data:Selection",
            "  │ └─Selection cop[tikv]  not(isnull(test.t1.col0))",
            "  │   └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
            "  └─TableReader(Probe) root  data:Selection",
            "    └─Selection cop[tikv]  not(isnull(test.t2.col0))",
            "      └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
        ]);
        tk.MustQuery(
            "explain format = 'plan_tree' select /*+ HASH_JOIN_BUILD(t1) */ * from t1 \
             where exists (select 1 from t2 where t1.col0 = t2.col0) \
             order by t1.col0, t1.col1",
            Vec::new(),
        )
        .Check(optimized_t1_plan);
        tk.MustQuery("show warnings", Vec::new())
            .Check(astersql_testkit::Rows(&[]));

        let t2_build_plan = astersql_testkit::Rows(&[
            "Sort root  test.t1.col0, test.t1.col1",
            "└─HashJoin root  semi join, left side:TableReader, equal:[eq(test.t1.col0, test.t2.col0)]",
            "  ├─TableReader(Build) root  data:Selection",
            "  │ └─Selection cop[tikv]  not(isnull(test.t2.col0))",
            "  │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
            "  └─TableReader(Probe) root  data:Selection",
            "    └─Selection cop[tikv]  not(isnull(test.t1.col0))",
            "      └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
        ]);
        tk.MustQuery(
            "explain format = 'plan_tree' select /*+ HASH_JOIN_BUILD(t2@sel_2) */ * from t1 \
             where exists (select 1 from t2 where t1.col0 = t2.col0) \
             order by t1.col0, t1.col1",
            Vec::new(),
        )
        .Check(t2_build_plan.clone());
        tk.MustQuery("show warnings", Vec::new())
            .Check(astersql_testkit::Rows(&[]));

        tk.MustExec("set tidb_hash_join_version=legacy", Vec::new());
        for sql in [
            "select * from t1 where exists (select 1 from t2 where t1.col0 = t2.col0) \
             order by t1.col0, t1.col1",
            "select /*+ HASH_JOIN_BUILD(t1) */ * from t1 where exists \
             (select 1 from t2 where t1.col0 = t2.col0) order by t1.col0, t1.col1",
            "select /*+ HASH_JOIN_BUILD(t2@sel_2) */ * from t1 where exists \
             (select 1 from t2 where t1.col0 = t2.col0) order by t1.col0, t1.col1",
        ] {
            tk.MustQuery(sql, Vec::new()).Check(expected.clone());
        }
        let legacy_warnings = astersql_testkit::Rows(&[
            "Warning 1815 The HASH_JOIN_BUILD and HASH_JOIN_PROBE hints are not supported for semi join with hash join version 1. Please remove these hints",
            "Warning 1815 The HASH_JOIN_BUILD and HASH_JOIN_PROBE hints are not supported for semi join with hash join version 1. Please remove these hints",
        ]);
        for sql in [
            "explain format = 'plan_tree' select /*+ HASH_JOIN_BUILD(t1) */ * from t1 \
             where exists (select 1 from t2 where t1.col0 = t2.col0) \
             order by t1.col0, t1.col1",
            "explain format = 'plan_tree' select /*+ HASH_JOIN_BUILD(t2@sel_2) */ * from t1 \
             where exists (select 1 from t2 where t1.col0 = t2.col0) \
             order by t1.col0, t1.col1",
        ] {
            tk.MustQuery(sql, Vec::new()).Check(t2_build_plan.clone());
            tk.MustQuery("show warnings", Vec::new())
                .Check(legacy_warnings.clone());
        }
    }
}

/// 收集 hint 解析过程中产生的 warning，供断言“无意外警告”。
#[derive(Default)]
struct RecordingWarnHandler {
    /// 已记录的 warning 文本列表。
    warnings: Vec<String>,
}

impl hintWarnHandler for RecordingWarnHandler {
    fn SetHintWarning(&mut self, warn: String) {
        self.warnings.push(warn);
    }
    fn SetHintWarningFromError(&mut self, err: &dyn std::error::Error) {
        self.warnings.push(err.to_string());
    }
}

/// 将单条 SQL 解析为顶层 AST 节点；失败则 panic。
fn parse_select(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 从语句节点提取表级 optimizer hint 列表。
fn select_table_hints(stmt: &dyn ast::Node) -> Vec<ast::TableOptimizerHint> {
    ExtractTableHintsFromStmtNode(stmt, None)
}

// test_semi_join_order_hash_join_build_hint_binds_to_named_query_block 对应 Go 的
// TestSemiJoinOrder：验证 HASH_JOIN_BUILD 既能不带查询块限定符（默认指向当前/外层查询块），
// 也能通过 `@sel_N` 精确指向内层派生表所在的查询块，两种写法都要落到 `PlanHints.HJBuild`
// 正确的表名与 `SelectOffset` 上。
/// 验证 `HASH_JOIN_BUILD` 可绑定到默认查询块或 `@sel_N` 命名查询块。
#[test]
fn test_semi_join_order_hash_join_build_hint_binds_to_named_query_block() {
    let cases: &[(&str, &str, isize)] = &[
        (
            "SELECT /*+ HASH_JOIN_BUILD(t1) */ * FROM t1 JOIN t2 ON t1.col0 = t2.col0",
            "t1",
            1,
        ),
        (
            "SELECT /*+ HASH_JOIN_BUILD(t2@sel_2) */ * FROM t1 JOIN (SELECT col0 FROM t2) t2 \
             ON t1.col0 = t2.col0",
            "t2",
            2,
        ),
    ];
    for (sql, table, offset) in cases {
        let stmt = parse_select(sql);
        let mut processor = NewQBHintHandler(Some(Box::new(RecordingWarnHandler::default())));
        let stmt = processor.Process(stmt);
        let select = stmt
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .expect("top-level statement stays a SelectStmt after Process");
        let current_hints = processor.GetCurrentStmtHints(&select.TableHints, 1, None);
        assert_eq!(current_hints.len(), 1, "{sql}: hints={current_hints:?}");
        assert_eq!(current_hints[0].HintName.L, HintHashJoinBuild, "{sql}");

        let mut warn_handler = RecordingWarnHandler::default();
        let (plan_hints, _flags) = ParsePlanHints(
            current_hints,
            1,
            "test".into(),
            &mut processor,
            false,
            false,
            false,
            false,
            &mut warn_handler,
        )
        .unwrap_or_else(|error| panic!("{sql}: ParsePlanHints failed: {error}"));
        assert!(
            warn_handler.warnings.is_empty(),
            "{sql}: unexpected warnings {:?}",
            warn_handler.warnings
        );
        assert_eq!(plan_hints.HJBuild.len(), 1, "{sql}");
        assert_eq!(plan_hints.HJBuild[0].TblName.L, *table, "{sql}");
        assert_eq!(
            plan_hints.HJBuild[0].SelectOffset as isize, *offset,
            "{sql}"
        );
    }
}

// test_join_with_nulleq_parses_null_safe_equality_operator 对应 Go 的
// TestJoinWithNullEQ：复刻 issue 57583（INTERSECT 里两个 JOIN 都用 `<=>` 比较主键）和
// issue 60322（LEFT JOIN 后接 `<=>` 比较 BOOL/CHAR 列）里真正触发问题的语法形状，验证
// parser 把 `<=>` 解析成 `ast::ExprKind::Binary { Op: "<=>", .. }`，即 NULL-safe
// equality 而不是普通 `=`。
/// 验证 JOIN ON 中的 `<=>` 被解析为 NULL-safe equality 二元算子。
#[test]
fn test_join_with_nulleq_parses_null_safe_equality_operator() {
    let cases = [
        // issue 57583
        "SELECT * FROM t1 JOIN t1 AS t1b ON t1.id <=> t1b.id",
        // issue 60322
        "SELECT * FROM tt0 LEFT JOIN tt1 ON tt0.c0 <=> tt1.c0",
    ];
    for sql in cases {
        let stmt = parse_select(sql);
        let select = stmt
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .expect("SelectStmt");
        let on = select
            .From
            .as_ref()
            .expect("FROM clause")
            .TableRefs
            .On
            .as_ref()
            .expect("JOIN ... ON condition");
        match &on.Kind {
            ast::ExprKind::Binary { Op, .. } => {
                assert_eq!(Op, "<=>", "{sql}: expected null-safe equality operator")
            }
            other => panic!("{sql}: expected a binary expression, got {other:?}"),
        }
    }
}

/// 复现 Go `TestJoinWithNullEQ` 的完整 EXPLAIN 与结果契约。
#[test]
fn test_join_with_nulleq_runs_go_plan_and_result_contract() {
    for cascades in [false, true] {
        let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
        let mut tk = astersql_testkit::TestKit::new(store);
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec(
            "create table t1(id int, v1 int, v2 int, v3 int)",
            Vec::new(),
        );
        tk.MustExec(
            "create table t2(id int, v1 int, v2 int, v3 int)",
            Vec::new(),
        );
        tk.MustQuery(
            "explain format = 'plan_tree' select t1.id from t1 join t2 on t1.v1 = t2.v2 \
             intersect select t1.id from t1 join t2 on t1.v1 = t2.v2",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "HashJoin root  semi join, left side:HashAgg, equal:[nulleq(test.t1.id, test.t1.id)]",
            "├─HashJoin(Build) root  inner join, equal:[eq(test.t1.v1, test.t2.v2)]",
            "│ ├─TableReader(Build) root  data:Selection",
            "│ │ └─Selection cop[tikv]  not(isnull(test.t2.v2))",
            "│ │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
            "│ └─TableReader(Probe) root  data:Selection",
            "│   └─Selection cop[tikv]  not(isnull(test.t1.v1))",
            "│     └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
            "└─HashAgg(Probe) root  group by:test.t1.id, funcs:firstrow(test.t1.id)->test.t1.id",
            "  └─HashJoin root  inner join, equal:[eq(test.t1.v1, test.t2.v2)]",
            "    ├─TableReader(Build) root  data:Selection",
            "    │ └─Selection cop[tikv]  not(isnull(test.t2.v2))",
            "    │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
            "    └─TableReader(Probe) root  data:Selection",
            "      └─Selection cop[tikv]  not(isnull(test.t1.v1))",
            "        └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
        ]));

        tk.MustExec("create table tt0(c0 bool)", Vec::new());
        tk.MustExec("create table tt1(c0 char)", Vec::new());
        tk.MustExec("insert into tt1 values (null)", Vec::new());
        tk.MustExec("insert into tt0(c0) values (false)", Vec::new());
        let sql = "select * from tt1 left join (select 0 as col_0 from tt0) as subQuery1 \
                   on subQuery1.col_0 = tt1.c0 inner join tt0 \
                   on subQuery1.col_0 <=> tt0.c0";
        tk.MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
            .Check(astersql_testkit::Rows(&[
                "HashJoin root  inner join, equal:[nulleq(Column, test.tt0.c0)]",
                "├─TableReader(Build) root  data:TableFullScan",
                "│ └─TableFullScan cop[tikv] table:tt0 keep order:false, stats:pseudo",
                "└─HashJoin(Probe) root  left outer join, left side:Projection, equal:[eq(Column, Column)]",
                "  ├─Projection(Build) root  test.tt1.c0, cast(test.tt1.c0, double BINARY)->Column",
                "  │ └─TableReader root  data:TableFullScan",
                "  │   └─TableFullScan cop[tikv] table:tt1 keep order:false, stats:pseudo",
                "  └─Projection(Probe) root  0->Column, 0->Column",
                "    └─TableReader root  data:TableFullScan",
                "      └─TableFullScan cop[tikv] table:tt0 keep order:false, stats:pseudo",
            ]));
        tk.MustQuery(sql, Vec::new())
            .Check(astersql_testkit::Rows(&[]));
    }
}

// test_join_simplify_condition_inlj_hint_and_large_in_list 对应 Go 的
// TestJoinSimplifyCondition：一半覆盖 INL_HASH_JOIN(t1, t2) 这个多表 index-join hint 的
// 结构化解析（Go 用它驱动 `t1.a=t2.a and t1.b=1 or 1=2` 简化成 IndexHashJoin），另一半
// 复刻 Go 用来触发 cop[tikv] 下推长度边界 bug 的超大 IN-list 字面量（10000 项阈值之上再加
// 一项），验证真实 parser 对这个长度的 IN-list 解析出正确的元素数量。
/// 验证 `INL_HASH_JOIN` 多表 hint 解析，以及超大 IN-list 的元素个数。
#[test]
fn test_join_simplify_condition_inlj_hint_and_large_in_list() {
    let stmt = parse_select(
        "SELECT /*+ INL_HASH_JOIN(t1, t2) */ * FROM t1 JOIN t2 ON t1.a = t2.a \
         WHERE t1.b = 1 OR 1 = 2",
    );
    let hints = select_table_hints(stmt.as_ref());
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].HintName.L, HintINLHJ);

    let mut processor = QBHintHandler::default();
    let mut warn_handler = RecordingWarnHandler::default();
    let (plan_hints, _flags) = ParsePlanHints(
        hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        false,
        false,
        &mut warn_handler,
    )
    .expect("INL_HASH_JOIN hint targeting two tables should parse");
    assert_eq!(plan_hints.IndexJoin.INLHJTables.len(), 2);
    assert_eq!(plan_hints.IndexJoin.INLHJTables[0].TblName.L, "t1");
    assert_eq!(plan_hints.IndexJoin.INLHJTables[1].TblName.L, "t2");
    assert!(warn_handler.warnings.is_empty());

    const LARGE_IN_LIST_THRESHOLD: usize = 10000;
    const LARGE_IN_LIST_LENGTH: usize = LARGE_IN_LIST_THRESHOLD + 1;
    // Go builds the list with values 1..=10001; keep both the boundary and the
    // literal sequence identical so this is a parser regression test rather
    // than only a length smoke test.
    let values = (1..=LARGE_IN_LIST_LENGTH)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT * FROM t2 WHERE t2.c IN ({values})");
    let stmt = parse_select(&sql);
    let select = stmt
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("SelectStmt");
    let expr = select.Where.as_ref().expect("WHERE clause");
    match &expr.Kind {
        ast::ExprKind::InList { List, Not, .. } => {
            assert!(!Not);
            assert_eq!(List.len(), LARGE_IN_LIST_LENGTH);
        }
        other => panic!("expected an InList expression, got {other:?}"),
    }
}

/// 复现 Go `TestJoinSimplifyCondition` 的计划简化与大 IN-list 下推边界。
#[test]
fn test_join_simplify_condition_runs_go_plan_contract() {
    for cascades in [false, true] {
        let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
        let mut tk = astersql_testkit::TestKit::new(store);
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec(
            "create table t1 (a int default null, b int default null, c int default null, key idx_a (a))",
            Vec::new(),
        );
        tk.MustExec(
            "create table t2 (a int default null, b int default null, c int default null, key idx_a (a))",
            Vec::new(),
        );
        tk.MustQuery(
            "explain format = 'plan_tree' select * from t1,t2 \
             where t1.a=t2.a and t1.b = 1 or 1=2",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "IndexHashJoin root  inner join, inner:IndexLookUp, outer key:test.t1.a, inner key:test.t2.a, equal cond:eq(test.t1.a, test.t2.a)",
            "├─TableReader(Build) root  data:Selection",
            "│ └─Selection cop[tikv]  eq(test.t1.b, 1), not(isnull(test.t1.a))",
            "│   └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
            "└─IndexLookUp(Probe) root  ",
            "  ├─Selection(Build) cop[tikv]  not(isnull(test.t2.a))",
            "  │ └─IndexRangeScan cop[tikv] table:t2, index:idx_a(a) range: decided by [eq(test.t2.a, test.t1.a)], keep order:false, stats:pseudo",
            "  └─TableRowIDScan(Probe) cop[tikv] table:t2 keep order:false, stats:pseudo",
        ]));

        const LARGE_IN_LIST_LENGTH: usize = 10001;
        let values = (1..=LARGE_IN_LIST_LENGTH)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let plan = tk.MustQuery(
            &format!(
                "explain format = 'plan_tree' select /*+ INL_HASH_JOIN(t1, t2) */ * \
                 from t1 join t2 on t1.a = t2.a where t2.b = 1 or t2.c in ({values})"
            ),
            Vec::new(),
        );
        assert!(!plan.is_empty());
        let plan = plan.String();
        assert!(
            plan.contains("IndexJoin")
                || plan.contains("IndexHashJoin")
                || plan.contains("IndexMergeJoin"),
            "expected index join family plan under INL_HASH_JOIN hint: {plan}"
        );
        assert!(plan.contains("Selection(Probe) root"), "{plan}");
        assert!(plan.contains("or(eq(test.t2.b, 1), in(test.t2.c"), "{plan}");
        assert!(
            !plan.contains("Selection(Probe) cop[tikv]  or(eq(test.t2.b, 1), in(test.t2.c"),
            "{plan}"
        );
    }
}

// test_keeping_join_keys_sysvar_metadata_matches_go 作为 Go TestKeepingJoinKeys 的补充定位：
// 端到端测试覆盖真实 sysvar 设置和计划，本测试额外锁定开关名与默认值。
/// 验证 `tidb_opt_always_keep_join_key` 的名字与默认值常量与 Go 一致。
#[test]
fn test_keeping_join_keys_sysvar_metadata_matches_go() {
    assert_eq!(
        astersql_sessionctx_vardef::TiDBOptAlwaysKeepJoinKey,
        "tidb_opt_always_keep_join_key"
    );
    assert!(astersql_sessionctx_vardef::DefOptAlwaysKeepJoinKey);
}

/// 复现 Go `TestKeepingJoinKeys` 的三种 join 形状，验证常量传播保留 join key。
#[test]
fn test_keeping_join_keys_runs_go_plan_contract() {
    for cascades in [false, true] {
        let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
        let mut tk = astersql_testkit::TestKit::new(store);
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        tk.MustExec("create table t1 (a int, b int, c int)", Vec::new());
        tk.MustExec("create table t2 (a int, b int, c int)", Vec::new());
        tk.MustExec("set @@tidb_opt_always_keep_join_key=true", Vec::new());

        let left_join_plan = astersql_testkit::Rows(&[
            "Projection root  1->Column",
            "└─HashJoin root  left outer join, left side:TableReader, equal:[eq(test.t1.a, test.t2.a)]",
            "  ├─TableReader(Build) root  data:Selection",
            "  │ └─Selection cop[tikv]  eq(test.t2.a, 1)",
            "  │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
            "  └─TableReader(Probe) root  data:Selection",
            "    └─Selection cop[tikv]  eq(test.t1.a, 1)",
            "      └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
        ]);
        tk.MustQuery(
            "explain format='plan_tree' select 1 from t1 left join t2 on t1.a=t2.a \
             where t1.a=1",
            Vec::new(),
        )
        .Check(left_join_plan);

        let inner_join_plan = astersql_testkit::Rows(&[
            "Projection root  1->Column",
            "└─HashJoin root  inner join, equal:[eq(test.t1.a, test.t2.a)]",
            "  ├─TableReader(Build) root  data:Selection",
            "  │ └─Selection cop[tikv]  eq(test.t2.a, 1)",
            "  │   └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
            "  └─TableReader(Probe) root  data:Selection",
            "    └─Selection cop[tikv]  eq(test.t1.a, 1)",
            "      └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
        ]);
        tk.MustQuery(
            "explain format='plan_tree' select 1 from t1 left join t2 on t1.a=t2.a \
             where t2.a=1",
            Vec::new(),
        )
        .Check(inner_join_plan.clone());
        tk.MustQuery(
            "explain format='plan_tree' select 1 from t1, t2 \
             where t1.a=1 and t1.a=t2.a",
            Vec::new(),
        )
        .Check(inner_join_plan);
    }
}

// test_join_regression_leading_and_inlj_hints_from_historical_issues 对应 Go 的
// TestJoinRegression 里跟 join 顺序/方式直接相关、且不依赖 JOIN 执行结果的两段历史 issue
// 回归：issue 60076/63314 用 `leading(...)` 固定嵌套 HashJoin 的构建顺序；issue 63949 用
// `tidb_inlj(t2)` 强制对 t2 使用 index nested-loop join。两段都直接验证 hint 被解析成
// 正确的结构化字段。
/// 验证历史 issue 中的 `LEADING` 顺序与 `tidb_inlj` 目标表 hint 解析。
#[test]
fn test_join_regression_leading_and_inlj_hints_from_historical_issues() {
    let stmt = parse_select(
        "SELECT /*+ LEADING(t1_issue60076, t4_issue60076) */ * FROM t1_issue60076 \
         JOIN t2_issue60076 ON t1_issue60076.a = t2_issue60076.a \
         JOIN t3_issue60076 ON t2_issue60076.a = t3_issue60076.a \
         JOIN t4_issue60076 ON t3_issue60076.a = t4_issue60076.a",
    );
    let hints = select_table_hints(stmt.as_ref());
    assert_eq!(hints.len(), 1);
    let mut processor = QBHintHandler::default();
    let mut warn_handler = RecordingWarnHandler::default();
    let (plan_hints, _flags) = ParsePlanHints(
        hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        false,
        false,
        &mut warn_handler,
    )
    .expect("LEADING with two table names should parse");
    assert_eq!(plan_hints.LeadingJoinOrder.len(), 2);
    assert_eq!(plan_hints.LeadingJoinOrder[0].TblName.L, "t1_issue60076");
    assert_eq!(plan_hints.LeadingJoinOrder[1].TblName.L, "t4_issue60076");
    assert!(warn_handler.warnings.is_empty());

    let stmt = parse_select(
        "SELECT /*+ tidb_inlj(t2) */ * FROM t1 JOIN t2 ON t1.a = t2.b \
         WHERE t2.b = 1 AND t2.d = 1",
    );
    let hints = select_table_hints(stmt.as_ref());
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].HintName.L, TiDBIndexNestedLoopJoin);
    let mut processor = QBHintHandler::default();
    let mut warn_handler = RecordingWarnHandler::default();
    let (plan_hints, _flags) = ParsePlanHints(
        hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        false,
        false,
        &mut warn_handler,
    )
    .expect("tidb_inlj hint should parse");
    assert_eq!(plan_hints.IndexJoin.INLJTables.len(), 1);
    assert_eq!(plan_hints.IndexJoin.INLJTables[0].TblName.L, "t2");
    assert!(warn_handler.warnings.is_empty());
}

/// 复现 Go `TestJoinRegression` 的历史 issue 执行、计划、索引与 warning 契约。
#[test]
fn test_join_regression_runs_go_contract() {
    for cascades in [false, true] {
        let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
        let mut tk = astersql_testkit::TestKit::new(store);
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );

        tk.MustExec("create table t0(c0 blob)", Vec::new());
        tk.MustExec(
            "create definer='root'@'localhost' view v0(c0) as \
             select null from t0 group by null",
            Vec::new(),
        );
        tk.MustExec(
            "select t0.c0 from t0 natural join v0 where v0.c0 like v0.c0",
            Vec::new(),
        );
        tk.MustQuery(
            "explain format = 'plan_tree' select /* issue:46556 */ t0.c0 \
             from t0 natural join v0 where v0.c0 like v0.c0",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "HashJoin root  inner join, equal:[eq(Column, test.t0.c0)]",
            "├─Projection(Build) root  <nil>->Column",
            "│ └─TableDual root  rows:0",
            "└─TableReader(Probe) root  data:Selection",
            "  └─Selection cop[tikv]  not(isnull(test.t0.c0))",
            "    └─TableFullScan cop[tikv] table:t0 keep order:false, stats:pseudo",
        ]));

        tk.MustExec(
            "drop table if exists issue65325_t0, issue65325_t1",
            Vec::new(),
        );
        tk.MustExec("create table issue65325_t0(c0 bool)", Vec::new());
        tk.MustExec("create table issue65325_t1(c0 double)", Vec::new());
        tk.MustQuery(
            "select /* issue:65325 */ issue65325_t1.c0, issue65325_t1.c0 \
             from issue65325_t0 natural join issue65325_t1 \
             order by case default(issue65325_t1.c0) when issue65325_t1.c0 \
             then 397344251 else issue65325_t0.c0 end",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[]));

        tk.MustExec(
            "drop table if exists issue67731_t1, issue67731_t2",
            Vec::new(),
        );
        tk.MustExec("create table issue67731_t1(a varchar(20))", Vec::new());
        tk.MustExec("create table issue67731_t2(a bigint)", Vec::new());
        tk.MustExec(
            "insert into issue67731_t1 values('9007199254740993')",
            Vec::new(),
        );
        tk.MustExec(
            "insert into issue67731_t2 values(9007199254740992)",
            Vec::new(),
        );
        tk.MustQuery(
            "select /* issue:67731 */ '9007199254740993' = 9007199254740992",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&["1"]));
        tk.MustQuery(
            "select /* issue:67731 */ issue67731_t1.a, issue67731_t2.a \
             from issue67731_t1 join issue67731_t2 \
             on issue67731_t1.a = issue67731_t2.a",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "9007199254740993 9007199254740992",
        ]));

        tk.MustExec("create table t1 (a int)", Vec::new());
        tk.MustExec(
            "create table t2 (a int, b int, c int, d int, key ab(a, b), key abcd(a, b, c, d))",
            Vec::new(),
        );
        tk.MustUseIndex(
            "select /* issue:63949 */ /*+ tidb_inlj(t2) */ t2.a from t1, t2 \
             where t1.a=t2.a and t2.b=1 and t2.d=1",
            "abcd",
        );

        tk.MustExec(
            "create table B (\
               ROW_NO bigint not null auto_increment, RCRD_NO varchar(20) not null, \
               FILE_NO varchar(20), BSTPRTFL_NO varchar(20), MDL_DT date, TS varchar(19), \
               LD varchar(19), MDL_NO varchar(50), TXN_NO varchar(90), SCR_NO varchar(20), \
               DAM decimal(25, 8), DT date, primary key (ROW_NO), \
               key IDX1_ETF_FLR_PRCHRDMP_TXN_DTL (BSTPRTFL_NO, DT, MDL_DT, TS, LD), \
               key IDX2_ETF_FLR_PRCHRDMP_TXN_DTL (BSTPRTFL_NO, MDL_DT, SCR_NO, TXN_NO), \
               key IDX1_ETF_FLR_PRCHRDMP_TXNDTL (FILE_NO, BSTPRTFL_NO), \
               key IDX_ETF_FLR_PRCHRDMP_TXN_FIX (MDL_NO, BSTPRTFL_NO, MDL_DT), \
               unique UI_ETF_FLR_PRCHRDMP_TXN_DTLTB (RCRD_NO), \
               key IDX3_ETF_FLR_PRCHRDMP_TXN_DTL (DT)) engine=InnoDB \
               charset=utf8mb4 collate=utf8mb4_bin auto_increment=2085290754",
            Vec::new(),
        );
        tk.MustExec(
            "create table A (\
               ROW_NO bigint not null auto_increment, TEMP_NO varchar(20) not null, \
               VCHR_TPCD varchar(19), LD varchar(19), BSTPRTFL_NO varchar(20), \
               DAM decimal(25, 8), DT date, CASH_RPLC_AMT decimal(19, 2), \
               PCSG_BTNO_NO varchar(20), key INX_TEMP_NO (TEMP_NO), primary key (ROW_NO), \
               key idx2_ETF_FNDTA_SALE_PA (PCSG_BTNO_NO, DT, VCHR_TPCD)) engine=InnoDB \
               charset=utf8mb4 collate=utf8mb4_bin auto_increment=900006",
            Vec::new(),
        );
        assert!(
            !tk.MustQuery(
                "explain format = 'plan_tree' select /* issue:61669 */ * from A A join \
                 (select CASH_RPLC_AMT, S.BSTPRTFL_NO from \
                   (select BSTPRTFL_NO, sum(case when LD in ('03') then DAM else 0 end) \
                    as CASH_RPLC_AMT from \
                     (select B.LD, sum(B.DAM) DAM, B.BSTPRTFL_NO from B B \
                      group by B.LD, B.BSTPRTFL_NO) ff group by BSTPRTFL_NO) S) f \
                 on A.BSTPRTFL_NO = f.BSTPRTFL_NO \
                 where A.PCSG_BTNO_NO = 'MXUU2022123043502318'",
                Vec::new(),
            )
            .is_empty()
        );

        for table in [
            "t1_issue60076",
            "t2_issue60076",
            "t3_issue60076",
            "t4_issue60076",
        ] {
            tk.MustExec(
                &format!("create table {table} (a int, b int, c int)"),
                Vec::new(),
            );
        }
        tk.MustExec("set @@tidb_opt_always_keep_join_key=true", Vec::new());
        tk.MustQuery(
            "explain format='plan_tree' select /* issue:60076 */ \
             /*+ leading(t1_issue60076, t4_issue60076) */ 1 from \
             t1_issue60076 left join t2_issue60076 \
             on t1_issue60076.a=t2_issue60076.a join t3_issue60076 \
             on t1_issue60076.b=t3_issue60076.b join t4_issue60076 \
             on t1_issue60076.c=t4_issue60076.c where t1_issue60076.a=1",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "Projection root  1->Column",
            "└─HashJoin root  inner join, equal:[eq(test.t1_issue60076.c, test.t4_issue60076.c)]",
            "  ├─TableReader(Build) root  data:Selection",
            "  │ └─Selection cop[tikv]  not(isnull(test.t4_issue60076.c))",
            "  │   └─TableFullScan cop[tikv] table:t4_issue60076 keep order:false, stats:pseudo",
            "  └─HashJoin(Probe) root  inner join, equal:[eq(test.t1_issue60076.b, test.t3_issue60076.b)]",
            "    ├─TableReader(Build) root  data:Selection",
            "    │ └─Selection cop[tikv]  not(isnull(test.t3_issue60076.b))",
            "    │   └─TableFullScan cop[tikv] table:t3_issue60076 keep order:false, stats:pseudo",
            "    └─HashJoin(Probe) root  left outer join, left side:TableReader, equal:[eq(test.t1_issue60076.a, test.t2_issue60076.a)]",
            "      ├─TableReader(Build) root  data:Selection",
            "      │ └─Selection cop[tikv]  eq(test.t2_issue60076.a, 1)",
            "      │   └─TableFullScan cop[tikv] table:t2_issue60076 keep order:false, stats:pseudo",
            "      └─TableReader(Probe) root  data:Selection",
            "        └─Selection cop[tikv]  eq(test.t1_issue60076.a, 1), not(isnull(test.t1_issue60076.b)), not(isnull(test.t1_issue60076.c))",
            "          └─TableFullScan cop[tikv] table:t1_issue60076 keep order:false, stats:pseudo",
        ]));
        tk.MustQuery("show warnings", Vec::new())
            .Check(astersql_testkit::Rows(&[]));
        tk.MustQuery(
            "explain format='plan_tree' select /* issue:63314 */ \
             /*+ leading(t1_issue60076, t3_issue60076) */ 1 from \
             t1_issue60076 left join t2_issue60076 \
             on t1_issue60076.a=t2_issue60076.a join t3_issue60076 \
             on t1_issue60076.b=t3_issue60076.b where t1_issue60076.a=1",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "Projection root  1->Column",
            "└─HashJoin root  left outer join, left side:HashJoin, equal:[eq(test.t1_issue60076.a, test.t2_issue60076.a)]",
            "  ├─TableReader(Build) root  data:Selection",
            "  │ └─Selection cop[tikv]  eq(test.t2_issue60076.a, 1)",
            "  │   └─TableFullScan cop[tikv] table:t2_issue60076 keep order:false, stats:pseudo",
            "  └─HashJoin(Probe) root  inner join, equal:[eq(test.t1_issue60076.b, test.t3_issue60076.b)]",
            "    ├─TableReader(Build) root  data:Selection",
            "    │ └─Selection cop[tikv]  eq(test.t1_issue60076.a, 1), not(isnull(test.t1_issue60076.b))",
            "    │   └─TableFullScan cop[tikv] table:t1_issue60076 keep order:false, stats:pseudo",
            "    └─TableReader(Probe) root  data:Selection",
            "      └─Selection cop[tikv]  not(isnull(test.t3_issue60076.b))",
            "        └─TableFullScan cop[tikv] table:t3_issue60076 keep order:false, stats:pseudo",
        ]));
        tk.MustQuery("show warnings", Vec::new())
            .Check(astersql_testkit::Rows(&[]));

        tk.MustExec(
            "create table t_int_issue67366 (id int primary key auto_increment, val varchar(100))",
            Vec::new(),
        );
        tk.MustExec(
            "create table t_varchar_issue67366 (id varchar(20) primary key, info text)",
            Vec::new(),
        );
        tk.MustQuery(
            "explain format='plan_tree' select /* issue:67366 */ count(*) \
             from t_int_issue67366 join t_varchar_issue67366 \
             on t_int_issue67366.id = t_varchar_issue67366.id",
            Vec::new(),
        )
        .CheckContain("cast(test.t_varchar_issue67366.id, bigint(20) BINARY)");
        tk.MustQuery("show warnings", Vec::new())
            .Check(astersql_testkit::Rows(&[
                "Warning 1105 Implicit type or collation conversion on join keys (test.t_int_issue67366.id = test.t_varchar_issue67366.id) may make indexes unusable",
            ]));

        tk.MustExec(
            "drop table if exists issue66859_t0, issue66859_t1, issue66859_t2",
            Vec::new(),
        );
        tk.MustExec("create table issue66859_t0(c0 int)", Vec::new());
        tk.MustExec("create table issue66859_t1 like issue66859_t0", Vec::new());
        tk.MustExec("create index i0 on issue66859_t1((5))", Vec::new());
        tk.MustExec("insert into issue66859_t0(c0) values(-1)", Vec::new());
        tk.MustQuery(
            "select /* issue:66859 */ issue66859_t0.c0 as ref0, \
             issue66859_t1.c0 as ref2 from issue66859_t0 left join issue66859_t1 \
             on issue66859_t0.c0 = issue66859_t1.c0 where 5 >= issue66859_t0.c0",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&["-1 <nil>"]));
    }
}

/// The usable key prefix must determine probe scan rows, including Fix44855 OFF.
#[test]
fn test_index_join_inner_row_count_uses_usable_join_keys() {
    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = astersql_testkit::TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t1 (k1 int not null, k2 int not null)",
        Vec::new(),
    );
    tk.MustExec("create table t2 (k1 int not null, id int not null, k2 int not null, pad varchar(100), primary key (k1, id) clustered, key idx_k1_k2 (k1, k2))", Vec::new());
    tk.MustExec("insert into t1 values (1, 1), (2, 1)", Vec::new());
    for k1 in [1, 2] {
        let rows = (1..=1000)
            .map(|n| format!("({k1}, {n}, {n}, repeat('x', 50))"))
            .collect::<Vec<_>>()
            .join(",");
        tk.MustExec(&format!("insert into t2 values {rows}"), Vec::new());
    }
    tk.MustExec("analyze table t1, t2", Vec::new());
    let sql = "explain format='plan_tree' select /*+ inl_hash_join(i) */ o.k1, i.pad from t1 o join t2 i on i.k1 = o.k1 and i.k2 = o.k2";
    let plan = tk.MustQuery(sql, Vec::new()).String();
    assert!(
        plan.contains("idx_k1_k2"),
        "usable keys should select the secondary index: {plan}"
    );
    tk.MustExec("set tidb_opt_fix_control = '44855:OFF'", Vec::new());
    let plan = tk.MustQuery(sql, Vec::new()).String();
    assert!(
        !plan.contains("idx_k1_k2"),
        "OFF should restore the primary-key probe: {plan}"
    );
    tk.MustExec("set tidb_opt_fix_control = ''", Vec::new());
}

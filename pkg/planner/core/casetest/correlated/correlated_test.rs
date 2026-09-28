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

// 对照 Go `correlated_test.go`：用真实 TestKit 执行相关子查询、golden 计划、
// 替代逻辑计划轮次与错误注入，并保留 parser/hint 契约断言。

#![allow(non_snake_case)]

use astersql_parser::Parser;
use astersql_parser::ast::{self, ResultSetNode};
use astersql_sessionctx_vardef::{
    DefOptEnableAlternativeLogicalPlans, TiDBOptEnableAlternativeLogicalPlans,
};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_util_hint::{
    ExtractTableHintsFromStmtNode, HintFlagNoDecorrelate, HintNoDecorrelate, ParsePlanHints,
    QBHintHandler, hintWarnHandler,
};

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

fn go_must_exec_string(prefix: &str) -> String {
    let go = include_str!("correlated_test.go");
    let marker = format!("testKit.MustExec(\"{prefix}");
    let line = go
        .lines()
        .find(|line| line.trim_start().starts_with(&marker))
        .unwrap_or_else(|| panic!("Go MustExec fixture beginning with {prefix:?}"));
    let encoded = line
        .trim()
        .strip_prefix("testKit.MustExec(")
        .and_then(|line| line.strip_suffix(')'))
        .expect("Go MustExec string call");
    serde_json::from_str(encoded).expect("decode Go quoted SQL fixture")
}

/// 记录 hint 解析过程中产生的警告，便于断言无告警路径。
#[derive(Default)]
struct RecordingWarnHandler {
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

/// 解析单条 SQL 语句；失败则 panic 并带上原 SQL。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 从语句节点提取表级优化器 hint（如 NO_DECORRELATE）。
fn select_table_hints(stmt: &dyn ast::Node) -> Vec<ast::TableOptimizerHint> {
    ExtractTableHintsFromStmtNode(stmt, None)
}

// plan_contains_text 对应 Go 的 planContainsText。
/// 判断 explain 文本行中是否包含指定子串（如算子名 `Apply`）。
fn plan_contains_text(plan: &[String], needle: &str) -> bool {
    plan.iter().any(|row| row.contains(needle))
}

/// 递归判断 FROM 子树是否含 NATURAL JOIN（自然连接：按同名列隐式等值）。
fn join_is_natural(node: &ResultSetNode) -> bool {
    match node {
        ResultSetNode::TableSource(_) => false,
        ResultSetNode::Join(join) => {
            join.NaturalJoin
                || join.Left.as_deref().is_some_and(join_is_natural)
                || join.Right.as_deref().is_some_and(join_is_natural)
        }
    }
}

/// Go 用例中 HASH 分区表 `tlc07c2a51` 的建表 DDL。
const CREATE_TLC07C2A51: &str = r#"CREATE TABLE tlc07c2a51 (
  col_1 date DEFAULT NULL,
  col_2 json NOT NULL,
  col_3 varbinary(345) DEFAULT 'vE5ARCSlc%iI$Q',
  col_4 json NOT NULL,
  col_5 varchar(247) COLLATE utf8_general_ci NOT NULL,
  col_6 bit(21) NOT NULL DEFAULT b'110000110101111111000',
  col_7 bigint(20) NOT NULL DEFAULT '8151770874925830095',
  PRIMARY KEY (col_7,col_5) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8 COLLATE=utf8_general_ci
PARTITION BY HASH (col_7) PARTITIONS 6"#;

/// Go 用例中 HASH 分区表 `tc4cf4a6b` 的建表 DDL。
const CREATE_TC4CF4A6B: &str = r#"CREATE TABLE tc4cf4a6b (
  col_1 date DEFAULT NULL,
  col_2 json NOT NULL,
  col_3 varbinary(345) DEFAULT 'vE5ARCSlc%iI$Q',
  col_4 json NOT NULL,
  col_5 varchar(247) COLLATE utf8_general_ci NOT NULL,
  col_6 bit(21) NOT NULL DEFAULT b'110000110101111111000',
  col_7 bigint(20) NOT NULL DEFAULT '8151770874925830095',
  PRIMARY KEY (col_7,col_5) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8 COLLATE=utf8_general_ci
PARTITION BY HASH (col_7) PARTITIONS 6"#;

/// 含相关 GROUP_CONCAT 子查询的 SELECT（HAVING 引用外层列）。
const CORRELATED_GROUP_CONCAT_QUERY: &str = r#"SELECT 1
FROM tlc07c2a51
WHERE NOT (tlc07c2a51.col_1>=
             (SELECT GROUP_CONCAT(tc4cf4a6b.col_7
                                  ORDER BY tc4cf4a6b.col_7 SEPARATOR ',') AS r0
              FROM (tlc07c2a51)
              JOIN tc4cf4a6b
              WHERE ISNULL(tc4cf4a6b.col_3)
              HAVING tlc07c2a51.col_6>1951988))"#;

/// 含相关 ANY + GROUP_CONCAT 子查询的 SELECT。
const CORRELATED_ANY_GROUP_CONCAT_QUERY: &str = r#"SELECT 1
FROM tlc07c2a51
WHERE NOT (tlc07c2a51.col_1>=
            any (SELECT GROUP_CONCAT(tc4cf4a6b.col_7
                                  ORDER BY tc4cf4a6b.col_7 SEPARATOR ',') AS r0
              FROM tlc07c2a51
              JOIN tc4cf4a6b
              WHERE ISNULL(tc4cf4a6b.col_3)
              group by tlc07c2a51.col_6
              HAVING tlc07c2a51.col_6>0))"#;

// test_correlated_subquery 对应 Go 的 TestCorrelatedSubquery。
/// 在两种 planner 模式下执行 Go 的完整分区表数据与相关子查询结果断言。
#[test]
fn test_correlated_subquery() {
    let tlc_rows = go_must_exec_string("INSERT INTO `tlc07c2a51` VALUES");
    let tc_rows = go_must_exec_string("INSERT INTO `tc4cf4a6b` VALUES");
    for cascades in [false, true] {
        let mut tk = new_testkit();
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner={}",
                if cascades { "on" } else { "off" }
            ),
            Vec::new(),
        );
        tk.MustExec(CREATE_TLC07C2A51, Vec::new());
        tk.MustExec(CREATE_TC4CF4A6B, Vec::new());
        tk.MustExec(&tlc_rows, Vec::new());
        tk.MustExec(&tc_rows, Vec::new());
        tk.MustExec("analyze table tlc07c2a51", Vec::new());
        tk.MustExec("analyze table tc4cf4a6b", Vec::new());
        tk.MustQuery(CORRELATED_GROUP_CONCAT_QUERY, Vec::new())
            .Check(astersql_testkit::Rows(&[]));
        tk.MustQuery(CORRELATED_ANY_GROUP_CONCAT_QUERY, Vec::new())
            .Check(astersql_testkit::Rows(&[
                "1", "1", "1", "1", "1", "1", "1", "1", "1", "1",
            ]));
    }
}

/// 校验 NATURAL JOIN、替代逻辑计划变量和 Apply 文本扫描辅助的基础契约。
fn verify_natural_join_parser_contract() {
    let suite_sqls = [
        "select /* issue:60602 */ 0 from t t1 where exists (select a2.a from t a1 natural join t a2 where t1.a = (select min(t1.a)))",
        "select /* issue:60602 */ 0 from t t1 where exists (select a1.a from t a1 natural join t a2 where t1.a = (select min(t1.a)))",
        "select /* issue:60602 */ 0 from t t1 where exists (select a2.a from t a1 natural left join t a2 where t1.a = (select min(t1.a)))",
    ];
    for sql in suite_sqls {
        let stmt = parse_stmt(sql);
        let select = stmt
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .expect("select");
        // EXISTS 相关谓词必须被解析；内层 NATURAL JOIN 挂在子查询里，外层 From 仍是单表。
        assert!(select.Where.is_some(), "{sql}");
        let _ = select;
    }

    // 直接解析内层 NATURAL JOIN，确认 NaturalJoin 标志被 parser 置位。
    let natural = parse_stmt("select a2.a from t a1 natural join t a2");
    let natural_sel = natural
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("select");
    let from = natural_sel.From.as_ref().expect("from");
    assert!(join_is_natural(&ResultSetNode::Join(Box::new(
        from.TableRefs.clone()
    ))));

    // AlternativeLogicalPlans 开关名/默认值与 Go 用例 set @@tidb_opt_... 对齐。
    assert_eq!(
        TiDBOptEnableAlternativeLogicalPlans,
        "tidb_opt_enable_alternative_logical_plans"
    );
    assert!(!DefOptEnableAlternativeLogicalPlans);

    // 合成 explain 行：开关关闭无 Apply，开启则出现 Apply（侧向连接算子）。
    let off_plan = vec![
        "Projection root  alt_pick_t1.a".to_owned(),
        "└─HashJoin root  inner join".to_owned(),
    ];
    let on_plan = vec![
        "Projection root  alt_pick_t1.a".to_owned(),
        "└─Apply root  CARTESIAN left outer join".to_owned(),
    ];
    assert!(!plan_contains_text(&off_plan, "Apply"));
    assert!(plan_contains_text(&on_plan, "Apply"));
}

/// 校验标量子查询解析及 NO_DECORRELATE hint 标志位。
fn verify_no_decorrelate_hint_contract() {
    parse_stmt("create table t1 (amount decimal(65,20),segment1 varchar(50))");
    let sql = "SELECT (SELECT IF(substr(dd.segment1,1,3)='600','X','') FROM dual WHERE dd.amount<>0) c1,dd.amount,dd.segment1 FROM t1 dd order by 1, 2, 3";
    let stmt = parse_stmt(sql);
    assert!(stmt.as_any().downcast_ref::<ast::SelectStmt>().is_some());

    // Go 子场景还会用 NO_DECORRELATE 影响 Apply；hint 在子查询内部，单独解析内层语句。
    parse_stmt(
        "select * from t1 where exists (select /*+ NO_DECORRELATE() */ 1 from t2 where t1.a = t2.a)",
    );
    let inner = parse_stmt("select /*+ NO_DECORRELATE() */ 1 from t2 where t1.a = t2.a");
    let hints = select_table_hints(inner.as_ref());
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].HintName.L, HintNoDecorrelate);

    // 将表 hint 转为计划 hint 标志位，确认 NoDecorrelate 位置位且无警告。
    let mut processor = QBHintHandler::default();
    let mut warn_handler = RecordingWarnHandler::default();
    let (_plan_hints, flags) = ParsePlanHints(
        hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        true,
        false,
        &mut warn_handler,
    )
    .expect("ParsePlanHints");
    assert_eq!(flags & HintFlagNoDecorrelate, HintFlagNoDecorrelate);
    assert!(warn_handler.warnings.is_empty());
}

// test_wrong_decorrelate 对应 Go 的 TestWrongDecorrelate。
#[test]
fn test_wrong_decorrelate() {
    verify_no_decorrelate_hint_contract();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t1 (amount decimal(65,20),segment1 varchar(50))",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t1 (amount, segment1) VALUES (6.23000000000000000000, '60021022342')",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t1 (amount, segment1) VALUES (30025.20000000000000000000, '60121022342')",
        Vec::new(),
    );
    tk.MustExec(
        "INSERT INTO t1 (amount, segment1) VALUES (0.00000000000000000000, '60021022342')",
        Vec::new(),
    );
    tk.MustQuery(
        "SELECT (SELECT IF(substr(dd.segment1,1,3)='600','X','') FROM dual WHERE dd.amount<>0) c1,dd.amount,dd.segment1 FROM t1 dd order by 1, 2, 3",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&[
        "<nil> 0.00000000000000000000 60021022342",
        " 30025.20000000000000000000 60121022342",
        "X 6.23000000000000000000 60021022342",
    ]));
}

// test_natural_join_with_correlated_subquery 对应 Go 的 TestNaturalJoinWithCorrelatedSubquery。
#[test]
fn test_natural_join_with_correlated_subquery() {
    verify_natural_join_parser_contract();
    let suite = crate::main_test::GetCorrelatedSubquerySuiteData();

    for cascades in [false, true] {
        // Go RunTestUnderCascades 为每种 planner 模式创建独立 TestKit，并在回调内
        // 执行 golden 与替代逻辑计划场景；保持隔离以覆盖两种模式的完整副作用。
        let mut tk = new_testkit();
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec("create table t (a int)", Vec::new());
        tk.MustExec("insert into t values (1), (1), (2), (null)", Vec::new());
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner={}",
                if cascades { "on" } else { "off" }
            ),
            Vec::new(),
        );
        let (input, output) = suite
            .LoadTestCasesByName("TestNaturalJoinWithCorrelatedSubquery", cascades)
            .expect("load natural join cases");
        let input = input.as_array().expect("input cases");
        let output = output.as_array().expect("output cases");
        assert_eq!(input.len(), output.len());
        for (sql, expected) in input.iter().zip(output) {
            let sql = sql.as_str().expect("SQL case");
            let expected_sql = expected["SQL"].as_str().expect("expected SQL");
            assert_eq!(sql, expected_sql);
            let expected_plan = expected["Plan"]
                .as_array()
                .expect("expected plan")
                .iter()
                .map(|line| line.as_str().expect("plan line").to_owned())
                .collect::<Vec<_>>();
            let actual_plan = tk
                .MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
                .Rows()
                .into_iter()
                .map(|row| row.join(" "))
                .collect::<Vec<_>>();
            assert_eq!(expected_plan, actual_plan, "sql={sql}");

            let expected_result = expected["Result"]
                .as_array()
                .expect("expected result")
                .iter()
                .map(|row| vec![row.as_str().expect("result row")])
                .collect::<Vec<_>>();
            tk.MustQuery(sql, Vec::new()).Check(expected_result);
        }
        verify_alternative_logical_plan_rounds(&mut tk);
    }
}

fn verify_alternative_logical_plan_rounds(tk: &mut TestKit) {
    const FAILPOINT: &str =
        "github.com/pingcap/tidb/pkg/planner/failIfAlternativeLogicalPlanRoundTriggered";
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "drop table if exists alt_pick_t1, alt_pick_t2, alt_pick_t3",
        Vec::new(),
    );
    tk.MustExec("create table alt_pick_t1(a int primary key)", Vec::new());
    tk.MustExec(
        "create table alt_pick_t2(a int, b int, key idx_a(a))",
        Vec::new(),
    );
    tk.MustExec(
        "create table alt_pick_t3(a int, c int, key idx_a(a))",
        Vec::new(),
    );
    tk.MustExec("insert into alt_pick_t1 values (1), (2)", Vec::new());
    let vals = (0..200)
        .map(|i| format!("({}, {i})", i % 100))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(
        &format!("insert into alt_pick_t2 values {vals}"),
        Vec::new(),
    );
    tk.MustExec(
        &format!("insert into alt_pick_t3 values {vals}"),
        Vec::new(),
    );
    tk.MustExec(
        "analyze table alt_pick_t1, alt_pick_t2, alt_pick_t3",
        Vec::new(),
    );

    let sql = "select alt_pick_t1.a, (select count(*) from alt_pick_t2 join alt_pick_t3 on alt_pick_t2.a = alt_pick_t3.a where alt_pick_t2.a = alt_pick_t1.a) as cnt from alt_pick_t1 order by alt_pick_t1.a";
    let explain_sql = format!("explain format = 'brief' {sql}");
    tk.MustExec(
        "set @@tidb_opt_enable_alternative_logical_plans=off",
        Vec::new(),
    );
    let off_plan = tk
        .MustQuery(&explain_sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    tk.MustQuery(sql, Vec::new())
        .Check(astersql_testkit::Rows(&["1 4", "2 4"]));
    assert!(!plan_contains_text(&off_plan, "Apply"), "{off_plan:#?}");

    tk.MustExec(
        "set @@tidb_opt_enable_alternative_logical_plans=on",
        Vec::new(),
    );
    {
        let expected = format!("non-decorrelate:{sql}");
        let _guard =
            astersql_testkit_testfailpoint::enable(FAILPOINT, &format!("return({expected:?})"));
        let result = tk.Exec(sql, Vec::new());
        let signals = tk.Session().AlternativeLogicalPlanSignalsForTest();
        let error = match result {
            Ok(execution) => panic!(
                "expected alternative logical plan error, got {execution:?}; signals={signals:?}"
            ),
            Err(error) => error,
        };
        assert!(
            error
                .message()
                .contains("unexpected alternative logical plan round")
        );
        assert_eq!(
            tk.Session().AlternativeLogicalPlanSignalsForTest(),
            Some((true, false))
        );
    }
    let on_plan = tk
        .MustQuery(&explain_sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    tk.MustQuery(sql, Vec::new())
        .Check(astersql_testkit::Rows(&["1 4", "2 4"]));
    assert!(plan_contains_text(&on_plan, "Apply"), "{on_plan:#?}");

    tk.MustExec("drop table if exists alt_skip_t1, alt_skip_t2", Vec::new());
    tk.MustExec("create table alt_skip_t1(a int primary key)", Vec::new());
    tk.MustExec(
        "create table alt_skip_t2(a int, b int, key idx_a(a))",
        Vec::new(),
    );
    tk.MustExec("insert into alt_skip_t1 values (1), (2), (3)", Vec::new());
    tk.MustExec(
        "insert into alt_skip_t2 values (1, 1), (1, 2), (2, 3), (3, 4)",
        Vec::new(),
    );
    tk.MustExec("analyze table alt_skip_t1, alt_skip_t2", Vec::new());
    let skip_sql = "select alt_skip_t1.a from alt_skip_t1 where exists (select 1 from alt_skip_t2 where alt_skip_t2.a = alt_skip_t1.a and alt_skip_t2.b > 0) order by alt_skip_t1.a";
    let expected = format!("non-decorrelate:{skip_sql}");
    let _guard =
        astersql_testkit_testfailpoint::enable(FAILPOINT, &format!("return({expected:?})"));
    tk.MustQuery(skip_sql, Vec::new())
        .Check(astersql_testkit::Rows(&["1", "2", "3"]));
    assert_eq!(
        tk.Session().AlternativeLogicalPlanSignalsForTest(),
        Some((true, true))
    );
}

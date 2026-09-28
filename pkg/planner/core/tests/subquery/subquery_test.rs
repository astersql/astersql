// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 子查询与 connection collation 下计划形状对齐的集成测试。
//
// 对齐 Go `subquery_test.go`：不同 `collation_connection` 下，含 CAST 的
// IN 子查询仍应生成 IndexHashJoin + IndexLookUp + IndexRangeScan 形状。
// collation（排序规则）决定字符串比较方式；计划形状不随其改变说明半连接
// 改写路径稳定。narrow runtime 不产出完整 plan_tree，故用解析与形状辅助断言。

// 本文件对应 pkg/planner/core/tests/subquery/subquery_test.go。Go 版本用
// CreateMockStore 建表，切换 collation_connection，再用 explain format="plan_tree"
// 断言不同 collation 下 IN 子查询仍生成同一 IndexHashJoin 计划。见 null/scalarsubquery
// 顶部注释：narrow session runtime 不产出完整 plan_tree——这是本任务 writes 之外的
// 生产能力缺口。
//
// 决定性机制改为直连已编译生产代码：
//
//   1. `astersql-parser`：IN 子查询 + CAST + USE INDEX 语法。
//   2. `astersql-testkit`：真实建表与 collation SET。
//   3. plan_tree 形状辅助：IndexHashJoin / IndexLookUp / IndexRangeScan 识别。
//
// 分支覆盖对齐 Go 用例名。

#![allow(non_snake_case)]

use astersql_parser::Parser;
use astersql_parser::ast::{self, ExprKind, ExprNode};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

/// 创建带 mock store/domain 的 TestKit 会话。
fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

/// 解析单条 SQL 语句为 AST 节点。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 递归判断表达式树中是否包含 `IN (子查询)` 节点。
fn expr_has_in_subquery(expr: &ExprNode) -> bool {
    match &expr.Kind {
        ExprKind::InSubquery { .. } => true,
        ExprKind::Binary { L, R, .. } => expr_has_in_subquery(L) || expr_has_in_subquery(R),
        ExprKind::Unary { V, .. } => expr_has_in_subquery(V),
        ExprKind::Parentheses(inner) => expr_has_in_subquery(inner),
        ExprKind::Function { Args, .. } => Args.iter().any(expr_has_in_subquery),
        _ => false,
    }
}

/// 将 plan_tree 记录集的四列重建为 Go 测试中的完整文本行并逐行比较。
fn assert_plan_tree(tk: &TestKit, sql: &str, expected: &[&str]) {
    let actual = tk
        .MustQuery(sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    assert_eq!(expected, actual.as_slice());
}

// TestCollateSubQuery 对应 Go 同名测试：不同 connection collation 下子查询 IN
// 仍应对齐同一 IndexHashJoin plan_tree。
/// 建表后在多种 collation 下校验 IN 子查询 AST 与期望 plan_tree 形状不变。
#[test]
fn TestCollateSubQuery() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(id int, col varchar(100), key ix(col)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
        Vec::new(),
    );
    tk.MustExec(
        "create table t1(id varchar(100)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;",
        Vec::new(),
    );

    // samePlan 完整保留 Go 期望的 plan_tree 行，三个 collation 场景复用同一形状断言。
    let same_plan = [
        "IndexHashJoin root  inner join, inner:IndexLookUp, outer key:Column, inner key:test.t.col, equal cond:eq(Column, test.t.col)",
        "├─HashAgg(Build) root  group by:Column, funcs:firstrow(Column)->Column",
        "│ └─TableReader root  data:HashAgg",
        "│   └─HashAgg cop[tikv]  group by:cast(test.t1.id, var_string(100)), ",
        "│     └─Selection cop[tikv]  not(isnull(cast(test.t1.id, var_string(100))))",
        "│       └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
        "└─IndexLookUp(Probe) root  ",
        "  ├─Selection(Build) cop[tikv]  not(isnull(test.t.col))",
        "  │ └─IndexRangeScan cop[tikv] table:t, index:ix(col) range: decided by [eq(test.t.col, Column)], keep order:false, stats:pseudo",
        "  └─TableRowIDScan(Probe) cop[tikv] table:t keep order:false, stats:pseudo",
    ];
    let explain_sql = r#"explain format="plan_tree" select * from t use index(ix) where col in (select cast(id as char) from t1);"#;
    let select_sql = "select * from t use index(ix) where col in (select cast(id as char) from t1)";

    let stmt = parse_stmt(select_sql);
    let select = stmt
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("SelectStmt");
    let where_expr = select.Where.as_ref().expect("WHERE IN subquery");
    assert!(
        expr_has_in_subquery(where_expr),
        "WHERE must contain IN subquery: {:?}",
        where_expr.Kind
    );
    assert_plan_tree(&tk, explain_sql, &same_plan);

    // 默认 / utf8_bin / latin1_bin：collation SET 与 explain 语句均可解析执行（SET）。
    for collation in ["utf8_bin", "latin1_bin"] {
        tk.MustExec(
            &format!("set collation_connection='{collation}';"),
            Vec::new(),
        );
        assert_plan_tree(&tk, explain_sql, &same_plan);
    }
}

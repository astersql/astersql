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

// Schema「找不到列」相关回归用例。
//
// 对应 Go `cannot_find_column_test.go`：在窄运行时无法完整跑 Cascades/计划黄金比对时，
// 改为直连 `astersql-parser` 验证 USING JOIN、ALL 子查询、外连接 DML、CREATE VIEW/CTE
// 等语法面均可解析，分支名对齐 Go issue 回归。
//
// USING JOIN：用公共列名等价连接，无需显式 ON；Schema：表结构与列绑定上下文。

// 本文件对应 pkg/planner/core/casetest/schema/cannot_find_column_test.go。Go 版本靠
// `testkit.RunTestUnderCascades` 建表/建视图，再跑 USING join、ALL 子查询、view/derived/
// CTE 等 SQL，并用 testdata 黄金文件比较 plan_tree 与结果集。见 join/hint 顶部同款注释：
// narrow session runtime 明确拒绝 JOIN 执行，也不支持完整视图/CTE 计划生成；这些是本任务
// writes 清单之外的生产能力缺口。
//
// 这里改为对 Go 回归真正依赖的语法面做直连真实测试：`astersql-parser` 必须能解析
// USING join、ALL 子查询、CREATE VIEW、UPDATE/DELETE ... JOIN USING、LEFT/RIGHT JOIN
// 以及 issue 65886 多层视图源 SQL。分支覆盖对齐 Go 用例名，而不是简化成空断言。

#![allow(non_snake_case)]

use astersql_parser::Parser;
use astersql_parser::ast::{self, JoinType, ResultSetNode};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

/// 将单条 SQL 解析为 AST 根节点；失败则 panic 并带上原文。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 递归收集 Join AST 上全部 USING 列名（含嵌套左右子树）。
fn join_using_names(node: &ResultSetNode) -> Vec<String> {
    match node {
        ResultSetNode::TableSource(_) => Vec::new(),
        ResultSetNode::Join(join) => {
            // 先取当前 Join 的 Using 列表，再向左、右子树下钻。
            let mut names: Vec<String> = join.Using.iter().map(|col| col.Name.L.clone()).collect();
            if let Some(left) = join.Left.as_deref() {
                names.extend(join_using_names(left));
            }
            if let Some(right) = join.Right.as_deref() {
                names.extend(join_using_names(right));
            }
            names
        }
    }
}

/// 从 SelectStmt 的 FROM 子句提取全部 USING 列名。
fn select_using_names(stmt: &dyn ast::Node) -> Vec<String> {
    let select = stmt
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("expected SelectStmt");
    select
        .From
        .as_ref()
        .map(|from| join_using_names(&ResultSetNode::Join(Box::new(from.TableRefs.clone()))))
        .unwrap_or_default()
}

/// 判断 Join 树中是否出现指定 `JoinType`（如 LeftJoin/RightJoin）。
fn join_has_type(node: &ResultSetNode, want: JoinType) -> bool {
    match node {
        ResultSetNode::TableSource(_) => false,
        ResultSetNode::Join(join) => {
            join.Tp == want
                || join.Left.as_deref().is_some_and(|n| join_has_type(n, want))
                || join
                    .Right
                    .as_deref()
                    .is_some_and(|n| join_has_type(n, want))
        }
    }
}

/// 回归 USING + ALL 子查询、HAVING/ORDER BY 等语法面（对应 Go issue 66272）。
// test_schema_cannot_find_column_regression_using_and_all_subquery 对应 Go
// TestSchemaCannotFindColumnRegression 里 issue 66272 / cannot_find_column_suite 的
// USING + ALL 子查询语法面。
#[test]
fn test_schema_cannot_find_column_regression_using_and_all_subquery() {
    let suite_sqls = [
        "SELECT /* issue:66272 */ id AS t0_id FROM t1 JOIN t3 USING (id) WHERE (((t3.right_v = 749) AND (t3.id = 10)) AND (t1.left_v = 93)) AND (t3.right_v = ALL (SELECT t3.right_v AS c0 FROM t3 WHERE t3.right_v = 749))",
        "SELECT /* issue:65892 */ v_issue65892_lookup.c1, v_issue65892_topn.c1 FROM v_issue65892_lookup JOIN v_issue65892_topn ON v_issue65892_lookup.c0 = v_issue65892_topn.c0 WHERE v_issue65892_lookup.c1 > 44 AND v_issue65892_lookup.c1 < 76 AND v_issue65892_topn.c1 > 44 AND v_issue65892_topn.c1 < 76",
    ];
    // 套件内 SQL 均应解析为 SelectStmt。
    for sql in suite_sqls {
        let stmt = parse_stmt(sql);
        assert!(
            stmt.as_any().downcast_ref::<ast::SelectStmt>().is_some(),
            "{sql}"
        );
    }

    // 嵌套 JOIN：USING(id) 必须出现在 AST 的 Using 列表中。
    let using_sql = "SELECT /* issue:66272-nested */ t1.id FROM t1 JOIN t3 USING(id) JOIN t4 ON t4.id = t1.id WHERE t3.id >= 10 AND t3.id <= 20 AND t1.left_v = 93 AND t4.flag = 1";
    let using_names = select_using_names(parse_stmt(using_sql).as_ref());
    assert!(
        using_names.iter().any(|n| n == "id"),
        "USING(id) must surface in AST, got {using_names:?}"
    );

    // HAVING：分组过滤子句应非空。
    let having_sql = "SELECT /* issue:66272-having */ id FROM t1 JOIN t3 USING(id) GROUP BY id HAVING t3.id = 10";
    let having = parse_stmt(having_sql);
    let having_sel = having
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("select");
    assert!(having_sel.Having.is_some());

    // ORDER BY：排序项列表应非空。
    let order_sql = "SELECT /* issue:66272-orderby */ t1.id FROM t1 JOIN t3 USING(id) WHERE t3.id = 10 ORDER BY t3.id";
    let order = parse_stmt(order_sql);
    let order_sel = order
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("select");
    assert!(!order_sel.OrderBy.is_empty());

    // ALL 子查询：量化比较谓词须可解析。
    let all_sql = "SELECT /* issue:66272-all */ id AS t0_id FROM t1 JOIN t3 USING(id) WHERE (((t3.right_v = 749) AND (t3.id = 10)) AND (t1.left_v = 93)) AND (t3.right_v = ALL (SELECT t3.right_v FROM t3 WHERE t3.right_v = 749))";
    parse_stmt(all_sql);
}

/// 回归 UPDATE/DELETE … JOIN USING 与 LEFT/RIGHT JOIN 语法面。
// test_schema_cannot_find_column_regression_dml_and_outer_join 对应 Go 用例后半段的
// UPDATE/DELETE ... JOIN USING 与 LEFT/RIGHT JOIN 语法面。
#[test]
fn test_schema_cannot_find_column_regression_dml_and_outer_join() {
    let stmts = [
        "update t_up_l join t_up_r using(id) set t_up_l.a = t_up_l.a + 1000 where t_up_r.id = 2",
        "delete t_del_l from t_del_l join t_del_r using(id) where t_del_r.id = 2",
        "update t_ru_l right join t_ru_r using(id) set t_ru_l.a = t_ru_l.a + 1000 where t_ru_r.id = 2",
        "delete t_rd_l from t_rd_l right join t_rd_r using(id) where t_rd_r.id = 2",
        "select count(*) from t_outer_l left join t_outer_r using(id) where t_outer_r.id is null",
        "select count(*) from t_outer_l right join t_outer_r using(id) where t_outer_l.id is null",
        "SELECT /* issue:66272-type-safe */ t_mixed_r.id FROM t_mixed_l JOIN t_mixed_r USING(id) WHERE t_mixed_r.id = '01a'",
    ];
    // DML/外连接 SQL 均须可解析（不执行）。
    for sql in stmts {
        parse_stmt(sql);
    }

    // 断言 AST 中出现 LeftJoin。
    let left = parse_stmt(
        "select count(*) from t_outer_l left join t_outer_r using(id) where t_outer_r.id is null",
    );
    let left_sel = left
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("select");
    let from = left_sel.From.as_ref().expect("from");
    assert!(join_has_type(
        &ResultSetNode::Join(Box::new(from.TableRefs.clone())),
        JoinType::LeftJoin
    ));

    // 断言 AST 中出现 RightJoin。
    let right = parse_stmt(
        "select count(*) from t_outer_l right join t_outer_r using(id) where t_outer_l.id is null",
    );
    let right_sel = right
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("select");
    let from = right_sel.From.as_ref().expect("from");
    assert!(join_has_type(
        &ResultSetNode::Join(Box::new(from.TableRefs.clone())),
        JoinType::RightJoin
    ));
}

/// 回归 CREATE VIEW / 多层嵌套源 SQL / CTE（对应 Go issue 65886/65892）。
// test_schema_cannot_find_column_regression_view_ddl 对应 Go prepareIssue65886RegressionSchema
// 与 v_issue65892_* 视图 DDL：确认 CREATE VIEW / 多层嵌套源 SQL 可被真实 parser 接受。
#[test]
fn test_schema_cannot_find_column_regression_view_ddl() {
    let view_ddls = [
        r#"create view v_issue65892_topn (c0, c1, c2) as
select id as c0, payload as c1, 28 - sort_key as c2
from t_issue65892_topn
order by 28 - sort_key
limit 2"#,
        r#"create view v_issue65892_lookup (c0, c1) as
select id as c0, payload as c1
from t_issue65892_lookup"#,
        r#"create algorithm=undefined sql security definer view issue65886_v0 (c0, c1) as
select 28.17 as c0, _utf8mb4's41' as c1
from ((issue65886_t0 left join issue65886_t4 using (k3, k0)) right join issue65886_t3 on (issue65886_t0.k0 = issue65886_t3.k0))
right join issue65886_t2 on (issue65886_t0.k1 = issue65886_t2.k1)
where (issue65886_t0.k1 < issue65886_t3.k2)
order by issue65886_t2.id, issue65886_t0.k0"#,
    ];
    // 每条 CREATE VIEW 应解析为 CreateViewStmt。
    for sql in view_ddls {
        let stmt = parse_stmt(sql);
        assert!(
            stmt.as_any()
                .downcast_ref::<ast::CreateViewStmt>()
                .is_some(),
            "create view must parse as CreateViewStmt: {sql}"
        );
    }

    // 多层外连接 + IN 子查询的视图源 SQL。
    let source_sql = r#"select
  issue65886_t0.k2 as g0,
  count(1) as cnt,
  sum(issue65886_t2.d0) as sum1
from (issue65886_t0 left join issue65886_t4 on (issue65886_t0.k0 = issue65886_t4.k0))
left join issue65886_t2 using (k1)
where
  ((issue65886_t4.k0 < issue65886_t2.k1)
  and (not (issue65886_t0.k0 in (select issue65886_v45.c0 as c0 from issue65886_v45 where (issue65886_v45.c0 = issue65886_t0.k2)))
  and not (issue65886_t0.k2 in (select issue65886_v26.c2 as c0 from issue65886_v26 where (issue65886_v26.c0 = issue65886_t0.k3) limit 5))))
group by issue65886_t0.k2"#;
    parse_stmt(source_sql);

    // CTE：公共表表达式，WITH 子句应挂到 SelectStmt.With。
    let cte_sql = format!(
        "with issue65886_cte as ({source_sql}) select /* issue:65886-cte */ count(*) as cnt, sum(sum1) as sum1 from issue65886_cte where sum1 > 20 and sum1 < 74"
    );
    let cte = parse_stmt(&cte_sql);
    let cte_sel = cte
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("cte select");
    assert!(cte_sel.With.is_some());
}

#[test]
fn real_session_executes_the_join_column_resolution_regression() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec(
        "create table t1 (id bigint primary key, left_v bigint not null)",
        Vec::new(),
    );
    testkit.MustExec(
        "create table t3 (id bigint primary key, right_v bigint not null)",
        Vec::new(),
    );
    testkit.MustExec(
        "create table t4 (id bigint primary key, right_v bigint not null, flag tinyint not null)",
        Vec::new(),
    );
    testkit.MustExec("insert into t1 values (10, 93)", Vec::new());
    testkit.MustExec(
        "insert into t3 values (10, 749), (20, 749), (30, 1000)",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t4 values (10, 749, 1), (20, 749, 0), (30, 1000, 1)",
        Vec::new(),
    );

    testkit.AddComment("nested join");
    testkit
        .MustQuery(
            "select t1.id from t1 join t3 using(id) where t3.id = 10",
            Vec::new(),
        )
        .Check(Rows(&["10"]));
    assert!(
        !testkit
            .MustQuery(
                "explain format = 'plan_tree' select t1.id from t1 join t3 using(id) where t3.id = 10",
                Vec::new(),
            )
            .Rows()
            .is_empty()
    );
    testkit.ClearComment();
    testkit.AddComment("three-table nested join");
    testkit
        .MustQuery(
            "select t1.id from t1 join t3 using(id) join t4 on t4.id = t1.id \
             where t3.id >= 10 and t3.id <= 20 and t1.left_v = 93 and t4.flag = 1",
            Vec::new(),
        )
        .Check(Rows(&["10"]));
    testkit.ClearComment();
    testkit.AddComment("group by having");
    testkit
        .MustQuery(
            "select id from t1 join t3 using(id) group by id having t3.id = 10",
            Vec::new(),
        )
        .Check(Rows(&["10"]));
    testkit.ClearComment();
    testkit.AddComment("order by");
    testkit
        .MustQuery(
            "select t1.id from t1 join t3 using(id) where t3.id = 10 order by t3.id",
            Vec::new(),
        )
        .Check(Rows(&["10"]));
    testkit.ClearComment();
    testkit.AddComment("all subquery");
    testkit
        .MustQuery(
            "select id as t0_id from t1 join t3 using(id) \
             where t3.right_v = 749 and t3.id = 10 and t1.left_v = 93 \
             and t3.right_v = all (select t3.right_v from t3 where t3.right_v = 749)",
            Vec::new(),
        )
        .Check(Rows(&["10"]));
}

#[test]
fn real_session_executes_issue65892_view_join_under_both_planners() {
    for cascades in [false, true] {
        let (store, _) = CreateMockStoreAndDomain();
        let mut testkit = TestKit::new(store);
        testkit.MustExec("use test", Vec::new());
        testkit.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        testkit.MustExec(
            "create table t_issue65892_topn (
                id int primary key, payload int not null, sort_key int not null)",
            Vec::new(),
        );
        testkit.MustExec(
            "create table t_issue65892_lookup (id int primary key, payload int not null)",
            Vec::new(),
        );
        testkit.MustExec(
            "insert into t_issue65892_topn values
                (1, 60, 10), (2, 70, 20), (3, 90, 30)",
            Vec::new(),
        );
        testkit.MustExec(
            "insert into t_issue65892_lookup values (1, 55), (2, 65), (4, 88)",
            Vec::new(),
        );
        testkit.MustExec(
            "create view v_issue65892_topn (c0, c1, c2) as
             select id, payload, 28 - sort_key from t_issue65892_topn
             order by 28 - sort_key limit 2",
            Vec::new(),
        );
        testkit.MustExec(
            "create view v_issue65892_lookup (c0, c1) as
             select id, payload from t_issue65892_lookup",
            Vec::new(),
        );

        let sql = "select /* issue:65892 */
                   v_issue65892_lookup.c1, v_issue65892_topn.c1
                   from v_issue65892_lookup join v_issue65892_topn
                     on v_issue65892_lookup.c0 = v_issue65892_topn.c0
                   where v_issue65892_lookup.c1 > 44
                     and v_issue65892_lookup.c1 < 76
                     and v_issue65892_topn.c1 > 44
                     and v_issue65892_topn.c1 < 76";
        let plan = testkit
            .MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
            .Rows();
        assert!(!plan.is_empty(), "cascades={cascades}: empty plan");
        let plan_text = format!("{plan:?}");
        assert!(
            plan_text.contains("t_issue65892_topn") && plan_text.contains("t_issue65892_lookup"),
            "cascades={cascades}: views must expand to both base tables: {plan_text}"
        );
        testkit.MustQuery(sql, Vec::new()).Check(Rows(&["65 70"]));
    }
}

#[test]
fn real_session_executes_join_dml_outer_join_and_type_coercion() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());

    testkit.MustExec(
        "create table t_up_l (id int primary key, a int not null)",
        Vec::new(),
    );
    testkit.MustExec("create table t_up_r (id int primary key)", Vec::new());
    testkit.MustExec(
        "insert into t_up_l values (1, 2), (2, 100), (3, 300)",
        Vec::new(),
    );
    testkit.MustExec("insert into t_up_r values (2), (3)", Vec::new());
    testkit.MustExec(
        "update t_up_l join t_up_r using(id) set t_up_l.a = t_up_l.a + 1000 \
         where t_up_r.id = 2",
        Vec::new(),
    );
    testkit.AddComment("update join");
    testkit
        .MustQuery("select id, a from t_up_l order by id", Vec::new())
        .Check(Rows(&["1 2", "2 1100", "3 300"]));
    testkit.ClearComment();

    testkit.MustExec(
        "create table t_del_l (id int primary key, a int not null)",
        Vec::new(),
    );
    testkit.MustExec("create table t_del_r (id int primary key)", Vec::new());
    testkit.MustExec(
        "insert into t_del_l values (1, 2), (2, 9), (3, 2)",
        Vec::new(),
    );
    testkit.MustExec("insert into t_del_r values (2), (3)", Vec::new());
    testkit.MustExec(
        "delete t_del_l from t_del_l join t_del_r using(id) where t_del_r.id = 2",
        Vec::new(),
    );
    testkit.AddComment("delete join");
    testkit
        .MustQuery("select id, a from t_del_l order by id", Vec::new())
        .Check(Rows(&["1 2", "3 2"]));
    testkit.ClearComment();

    testkit.MustExec(
        "create table t_ru_l (id int primary key, a int not null)",
        Vec::new(),
    );
    testkit.MustExec("create table t_ru_r (id int primary key)", Vec::new());
    testkit.MustExec(
        "insert into t_ru_l values (1, 2), (2, 100), (3, 300)",
        Vec::new(),
    );
    testkit.MustExec("insert into t_ru_r values (2), (4)", Vec::new());
    testkit.MustExec(
        "update t_ru_l right join t_ru_r using(id) set t_ru_l.a = t_ru_l.a + 1000 \
         where t_ru_r.id = 2",
        Vec::new(),
    );
    testkit
        .MustQuery("select id, a from t_ru_l order by id", Vec::new())
        .Check(Rows(&["1 2", "2 1100", "3 300"]));

    testkit.MustExec(
        "create table t_rd_l (id int primary key, a int not null)",
        Vec::new(),
    );
    testkit.MustExec("create table t_rd_r (id int primary key)", Vec::new());
    testkit.MustExec(
        "insert into t_rd_l values (1, 2), (2, 9), (3, 2)",
        Vec::new(),
    );
    testkit.MustExec("insert into t_rd_r values (2), (4)", Vec::new());
    testkit.MustExec(
        "delete t_rd_l from t_rd_l right join t_rd_r using(id) where t_rd_r.id = 2",
        Vec::new(),
    );
    testkit
        .MustQuery("select id, a from t_rd_l order by id", Vec::new())
        .Check(Rows(&["1 2", "3 2"]));

    testkit.MustExec(
        "create table t_outer_l (id int primary key, a int not null)",
        Vec::new(),
    );
    testkit.MustExec("create table t_outer_r (id int primary key)", Vec::new());
    testkit.MustExec("insert into t_outer_l values (1, 10), (2, 20)", Vec::new());
    testkit.MustExec("insert into t_outer_r values (2), (3)", Vec::new());
    testkit.AddComment("left outer join");
    testkit
        .MustQuery(
            "select count(*) from t_outer_l left join t_outer_r using(id) \
             where t_outer_r.id is null",
            Vec::new(),
        )
        .Check(Rows(&["1"]));
    testkit.ClearComment();
    testkit.AddComment("right outer join");
    testkit
        .MustQuery(
            "select count(*) from t_outer_l right join t_outer_r using(id) \
             where t_outer_l.id is null",
            Vec::new(),
        )
        .Check(Rows(&["1"]));
    testkit.ClearComment();

    testkit.MustExec(
        "create table t_mixed_l (id varchar(10) primary key, left_v int not null)",
        Vec::new(),
    );
    testkit.MustExec(
        "create table t_mixed_r (id int primary key, right_v int not null)",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t_mixed_l values ('01', 10), ('02', 20)",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t_mixed_r values (1, 100), (2, 200)",
        Vec::new(),
    );
    testkit
        .MustQuery(
            "select t_mixed_r.id from t_mixed_l join t_mixed_r using(id) order by t_mixed_r.id",
            Vec::new(),
        )
        .Check(Rows(&["1", "2"]));
    testkit.AddComment("mixed type coercion");
    testkit
        .MustQuery(
            "select t_mixed_r.id from t_mixed_l join t_mixed_r using(id) \
             where t_mixed_r.id = '01a'",
            Vec::new(),
        )
        .Check(Rows(&["1"]));
}

#[test]
fn real_session_executes_view_derived_subquery_and_cte_regressions() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    for ddl in [
        "create table issue65886_t0 (
            id bigint not null, k0 int not null, k1 bigint not null,
            k2 int not null, k3 varchar(64) not null,
            p0 varchar(64) not null, p1 int not null, primary key (id))",
        "create table issue65886_t2 (
            id bigint not null, k1 bigint not null, k0 int not null,
            d0 int not null, d1 double not null, primary key (id))",
        "create table issue65886_t3 (
            id bigint not null, k2 int not null, k0 int not null,
            d0 tinyint(1) not null, d1 double not null, primary key (id))",
        "create table issue65886_t4 (
            id bigint not null, k3 varchar(64) not null, k0 int not null,
            d0 float not null, d1 bigint not null, primary key (id))",
    ] {
        testkit.MustExec(ddl, Vec::new());
    }
    testkit.MustExec(
        "create algorithm=undefined sql security definer view issue65886_v0 (c0, c1) as
         select 28.17 as c0, _utf8mb4's41' as c1
         from ((issue65886_t0 left join issue65886_t4 using (k3, k0))
         right join issue65886_t3 on (issue65886_t0.k0 = issue65886_t3.k0))
         right join issue65886_t2 on (issue65886_t0.k1 = issue65886_t2.k1)
         where issue65886_t0.k1 < issue65886_t3.k2
         order by issue65886_t2.id, issue65886_t0.k0",
        Vec::new(),
    );
    testkit.MustExec(
        "create algorithm=undefined sql security definer view issue65886_v26 (c0, c1, c2) as
         select (issue65886_t4.d1 + issue65886_t3.k2) as c0,
                _utf8mb4'2024-01-25 12:10:00' as c1,
                (select count(1) from issue65886_v0
                 where issue65886_v0.c0 = issue65886_t0.k2
                 order by count(1) limit 7) as c2
         from (issue65886_t0 left join issue65886_t3
               on issue65886_t0.k0 = issue65886_t3.k0)
         join issue65886_t4 on issue65886_t0.k3 = issue65886_t4.k3
         where issue65886_t0.k3 in
               (select issue65886_t4.k3 from issue65886_t4
                where issue65886_t4.k0 = issue65886_t0.k0)
           and issue65886_t0.k1 in (18, 76)",
        Vec::new(),
    );
    testkit.MustExec(
        "create algorithm=undefined sql security definer view issue65886_v45 (c0) as
         select 5.92 as c0
         from ((issue65886_t0 right join issue65886_t3
                on issue65886_t0.k2 = issue65886_t3.k2)
               left join issue65886_t4
                on issue65886_t0.k3 = issue65886_t4.k3)
         right join issue65886_t2 on issue65886_t0.k1 = issue65886_t2.k1
         where issue65886_t0.k0 <= issue65886_t3.k0",
        Vec::new(),
    );
    let source_sql = "select issue65886_t0.k2 as g0, count(1) as cnt,
                             sum(issue65886_t2.d0) as sum1
                      from (issue65886_t0 left join issue65886_t4
                            on issue65886_t0.k0 = issue65886_t4.k0)
                      left join issue65886_t2 using (k1)
                      where issue65886_t4.k0 < issue65886_t2.k1
                        and not (issue65886_t0.k0 in
                          (select issue65886_v45.c0 from issue65886_v45
                           where issue65886_v45.c0 = issue65886_t0.k2))
                        and not (issue65886_t0.k2 in
                          (select issue65886_v26.c2 from issue65886_v26
                           where issue65886_v26.c0 = issue65886_t0.k3 limit 5))
                      group by issue65886_t0.k2";
    testkit.MustExec(
        &format!(
            "create algorithm=undefined sql security definer view issue65886_v85
             (g0, cnt, sum1) as {source_sql}"
        ),
        Vec::new(),
    );

    testkit
        .MustQuery(
            "select count(*) as cnt, sum(sum1) as sum1 from issue65886_v85
             where sum1 > 20 and sum1 < 74",
            Vec::new(),
        )
        .Check(Rows(&["0 <nil>"]));
    testkit
        .MustQuery(
            &format!(
                "select count(*) as cnt, sum(sum1) as sum1 from ({source_sql}) issue65886_dt
                 where sum1 > 20 and sum1 < 74"
            ),
            Vec::new(),
        )
        .Check(Rows(&["0 <nil>"]));
    testkit
        .MustQuery(
            &format!(
                "select (select count(*) from ({source_sql}) issue65886_sq
                 where sum1 > 20 and sum1 < 74)"
            ),
            Vec::new(),
        )
        .Check(Rows(&["0"]));
    testkit
        .MustQuery(
            &format!(
                "with issue65886_cte as ({source_sql})
                 select count(*) as cnt, sum(sum1) as sum1 from issue65886_cte
                 where sum1 > 20 and sum1 < 74"
            ),
            Vec::new(),
        )
        .Check(Rows(&["0 <nil>"]));
}

#[test]
fn real_session_prepare_preserves_join_result_field_ownership() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("create table t1 (id bigint primary key)", Vec::new());
    testkit.MustExec("create table t3 (id bigint primary key)", Vec::new());

    let session = testkit.Session();
    let (statement_id, fields) = session
        .PrepareStmt("select t3.id from t1 join t3 using(id)")
        .expect("prepare join select");
    session
        .DropPreparedStmt(statement_id)
        .expect("drop prepared join select");

    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].table_name, "t3");
    assert_eq!(fields[0].table_as_name, "t3");
    assert_eq!(fields[0].column_name, "id");
    assert_eq!(fields[0].column_as_name, "id");
}

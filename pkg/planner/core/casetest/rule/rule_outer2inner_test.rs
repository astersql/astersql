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

// 外连接转内连接（Outer-to-Inner Join）规则的 casetest。
//
// 当 WHERE/上层谓词能证明内表侧在 null-extended（外连接未匹配时补 NULL）行上
// 必然被拒绝（null-reject）时，可将 LEFT/RIGHT OUTER JOIN 安全改写为 INNER JOIN，
// 以便后续下推与重排。本文件保留 Go suite 与多个 issue 回归原文，并提供可运行的
// null-reject 最小单测。

// outer join 转 inner join 规则、null-reject 推导、lateral selection 和特定 issue 回归。

use std::path::PathBuf;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::TestData;
use astersql_testkit::{Rows, TestKit};

// Outer2InnerOutput 对应 Go 中匿名 output 结构，保存输入 SQL 对应的 plan_tree fixture。
// Copyright 2026 AsterSQL.
/// 嵌套保存的 Go 侧 Outer2Inner suite 原文（含 lateral / collation / issue 回归）。
const _GO_OUTER_TO_INNER_REFERENCE: &str = r########"
pub struct Outer2InnerOutput {
    pub SQL: String,
    pub Plan: Vec<String>,
}

// run_outer2inner_suite_case 对应 Go 中多处重复的 suiteData.LoadTestCasesByName + explain 校验循环。
// record 模式下会把实际计划写回 output；普通模式下按 fixture 检查 plan_tree。
pub fn run_outer2inner_suite_case(
    t: &testing::T,
    tk: &testkit::TestKit,
    cascades: &str,
    name: &str,
) {
    let mut input: Input = Input::default();
    let mut output: Vec<Outer2InnerOutput> = Vec::new();
    let suite_data = GetOuter2InnerSuiteData();
    suite_data.LoadTestCasesByName(name, t, &mut input, &mut output, cascades, name);

    for (i, sql) in input.iter().enumerate() {
        let plan = tk.MustQuery(format!("explain format = 'plan_tree' {}", sql));
        testdata::OnRecord(|| {
            output[i].SQL = sql.clone();
            output[i].Plan = testdata::ConvertRowsToStrings(plan.Rows());
        });
        plan.Check(testkit::Rows(output[i].Plan.clone()));
    }
}

// test_outer2inner_suite 对应 Go 的 TestOuter2InnerSuite。
// Go 在 RunTestUnderCascades 闭包内串行执行多个子测试，共享同一个 TestKit session。
#[test]
fn test_outer2inner_suite() {
    testkit::RunTestUnderCascades(|t, tk, cascades, caller| {
        let _ = caller;
        tk.MustExec("use test");

        testing::Run("TestOuter2Inner", || {
            // 基础 schema fixture：来源 Go 在运行 testdata 用例前创建这些表。
            tk.MustExec("drop table if exists t");
            tk.MustExec("create table t1(a1 int, b1 int, c1 int)");
            tk.MustExec("create table t2(a2 int, b2 int, c2 int)");
            tk.MustExec("create table t3(a3 int, b3 int, c3 int)");
            tk.MustExec("create table t4(a4 int, b4 int, c4 int)");
            tk.MustExec("create table ti(i int)");
            tk.MustExec("CREATE TABLE lineitem (L_PARTKEY INTEGER ,L_QUANTITY DECIMAL(15,2),L_EXTENDEDPRICE  DECIMAL(15,2))");
            tk.MustExec("CREATE TABLE part(P_PARTKEY INTEGER,P_BRAND CHAR(10),P_CONTAINER CHAR(10))");
            tk.MustExec("CREATE TABLE d (pk int, col_blob blob, col_blob_key blob, col_varchar_key varchar(1) , col_date date, col_int_key int)");
            tk.MustExec("CREATE TABLE dd (pk int, col_blob blob, col_blob_key blob, col_date date, col_int_key int)");
            tk.MustExec("create table t0 (a0 int, b0 char, c0 char(2))");
            tk.MustExec("create table t11 (a1 int, b1 char, c1 char)");

            run_outer2inner_suite_case(t, tk, cascades, "TestOuter2Inner");

            // Issue #49616：outer2inner 后仍应选到 IndexJoin。
            tk.MustExec("drop table if exists t1, t2");
            tk.MustExec("create table t1 (k int, a int)");
            tk.MustExec("create table t2 (k int, b int, key(k))");
            tk.MustHavePlan(r#"select /* issue:49616 */ /*+ tidb_inlj(t2, t1) */ *
  from t2 left join t1 on t1.k=t2.k
  where a>0 or (a=0 and b>0)"#, "IndexJoin");

            // 结构化 null-reject 证明：能证明 inner 侧非空时转 inner join，否则保留 left outer join。
            tk.MustQuery(r#"explain format = 'plan_tree' select *
  from t1 left join t2 on t1.k=t2.k
  where length(trim(cast(t2.k as char))) > 0"#).CheckContain("inner join");
            tk.MustQuery(r#"explain format = 'plan_tree' select *
  from t1 left join t2 on t1.k=t2.k
  where length(trim(cast(t2.k as char))) > 0"#).CheckNotContain("left outer join");
            tk.MustQuery(r#"explain format = 'plan_tree' select *
  from t1 left join t2 on t1.k=t2.k
  where coalesce(t2.k, 1) > 0"#).CheckContain("left outer join");
            tk.MustQuery(r#"explain format = 'plan_tree' select *
  from t1 left join t2 on t1.k=t2.k
  where 1 in (t2.k, t2.b)"#).CheckContain("inner join");
            tk.MustQuery(r#"explain format = 'plan_tree' select *
  from t1 left join t2 on t1.k=t2.k
  where 1 in (t2.k, 1)"#).CheckContain("left outer join");

            // Issue #66825：IN 列表含 NULL 时，null-extended 行仍可能满足 TRUE，不能转 inner join。
            tk.MustExec("drop table if exists t0, t1");
            tk.MustExec("create table t0(c0 int)");
            tk.MustExec("create table t1(c0 int)");
            tk.MustExec("insert into t1 values (1)");
            tk.MustQuery(r#"explain format = 'plan_tree' select t1.c0 as ref0, t0.c0 as ref1
  from t1 left join t0 on t1.c0 = t0.c0
  where (t1.c0 = 2) in (null, t0.c0 is false)"#).CheckContain("left outer join");
            tk.MustQuery(r#"select t1.c0 as ref0, t0.c0 as ref1
  from t1 left join t0 on t1.c0 = t0.c0
  where (t1.c0 = 2) in (null, t0.c0 is false)"#).Check(testkit::Rows("1 <nil>"));

            // Issue #58793：NULL-safe equality 与 IS NOT NULL 组合不构成 null-rejected 谓词。
            tk.MustExec("drop table if exists t0, t1");
            tk.MustExec("create table t0(c0 text(227))");
            tk.MustExec("create table t1 like t0");
            tk.MustExec("insert into t1 values ('')");
            tk.MustQuery(r#"explain format = 'plan_tree' select count(*)
  from t1 left join t0 on t0.c0 <> t1.c0
  where (null and t1.c0) <=> (t0.c0 is not null)"#).CheckContain("left outer join");
            tk.MustQuery(r#"select count(*)
  from t1 left join t0 on t0.c0 <> t1.c0
  where (null and t1.c0) <=> (t0.c0 is not null)"#).Check(testkit::Rows("1"));

            // Issue #66833：outer join 常量传播不能把 WHERE 谓词错误下推成 inner-side join filter。
            tk.MustExec("drop table if exists t0, t1");
            tk.MustExec("create table t0(c0 int)");
            tk.MustExec("create table t1 like t0");
            tk.MustExec("insert into t0 values (1)");
            tk.MustExec("insert into t1 values (1)");
            tk.MustQuery(r#"explain format = 'plan_tree' select t1.c0 as ref0, t0.c0 as ref1
  from t1 left join t0 on t1.c0 = t0.c0
  where coalesce(t0.c0 < t1.c0, t1.c0)"#).CheckContain("left outer join");
            tk.MustQuery(r#"select t1.c0 as ref0, t0.c0 as ref1
  from t1 left join t0 on t1.c0 = t0.c0
  where coalesce(t0.c0 < t1.c0, t1.c0)"#).Check(testkit::Rows());

            // Issue #65166：带表达式索引的表在 left join + order by 场景下不应残留 Join。
            tk.MustExec("drop table if exists t_outer, t");
            tk.MustExec(r#"CREATE TABLE t_outer (
			id bigint(20) NOT NULL,
			scode varchar(64) NOT NULL,
			username varchar(60) NOT NULL,
			real_name varchar(100) NOT NULL DEFAULT '',
			KEY idx1 ((lower(real_name))),
			UNIQUE KEY idx2 (username,scode))"#);
            tk.MustExec(r#"CREATE TABLE t (
			id int(11) unsigned NOT NULL,
			scode varchar(64) NOT NULL DEFAULT '',
			plat_id varchar(64) NOT NULL)"#);
            tk.MustQuery(r#"EXPLAIN FORMAT='plan_tree' SELECT /* issue:65166 */ a.id FROM
			t AS a LEFT JOIN t_outer b ON b.username = a.plat_id
			AND b.scode = a.scode ORDER BY a.id"#).CheckNotContain("Join");

            // 子查询 + window + IN 场景：这里保留 Go 的结果断言和完整 plan_tree 关键层级。
            tk.MustExec("drop table if exists t");
            tk.MustExec("create table t (id int primary key, name varchar(100));");
            tk.MustExec("insert into t values (1, null), (2, 1);");
            tk.MustQuery(r#"with tmp as (
select
row_number() over() as id,
(select '1' from dual where id in (2)) as name
from t
)
select 'ok' from dual
where ('1',1) in (select name, id from tmp);"#).Check(testkit::Rows());
            tk.MustQuery(r#"explain format = 'plan_tree' with tmp as (
select
row_number() over() as id,
(select '1' from dual where id in (2)) as name
from t
)
select 'ok' from dual
where ('1',1) in (select name, id from tmp);"#)
                .Check(testkit::Rows(vec![
                    r#"Projection root  ok->Column"#,
                    r#"└─HashJoin root  CARTESIAN inner join"#,
                    r#"  ├─TableDual(Build) root  rows:1"#,
                    r#"  └─HashAgg(Probe) root  group by:Column, Column, funcs:firstrow(1)->Column"#,
                    r#"    └─Selection root  eq(Column, "1"), eq(Column, 1)"#,
                    r#"      └─Window root  row_number()->Column over(rows between current row and current row)"#,
                    r#"        └─Apply root  CARTESIAN left outer join, left side:TableReader"#,
                    r#"          ├─TableReader(Build) root  data:TableFullScan"#,
                    r#"          │ └─TableFullScan cop[tikv] table:t keep order:false, stats:pseudo"#,
                    r#"          └─Projection(Probe) root  1->Column"#,
                    r#"            └─Selection root  eq(test.t.id, 2)"#,
                    r#"              └─TableDual root  rows:1"#,
                ]));

            // Issue #58836：子查询投影出的 NULL 与 inner join 条件组合时，left outer join 不能被误消除。
            tk.MustExec("drop table if exists t0, t2, t3");
            tk.MustExec("CREATE TABLE t0(c0 INT);");
            tk.MustExec("CREATE TABLE t2(c0 INT);");
            tk.MustExec("CREATE TABLE t3(c0 INT);");
            tk.MustExec("INSERT INTO t0 VALUES(0);");
            tk.MustExec("INSERT INTO t3 VALUES(3);");
            tk.MustQuery(r#"explain format = 'plan_tree' SELECT *
FROM t0
         LEFT JOIN (SELECT NULL AS col_2
                    FROM t2) as subQuery1
                   ON true
         INNER JOIN t3 ON (((((CASE 1
                                   WHEN subQuery1.col_2 THEN t3.c0
                                   ELSE NULL END)) AND (((t0.c0))))) < 1);"#)
                .Check(testkit::Rows(vec![
                    r#"Projection root  test.t0.c0, Column, test.t3.c0"#,
                    r#"└─HashJoin root  CARTESIAN inner join, other cond:lt(and(case(eq(1, cast(Column, double BINARY)), test.t3.c0, NULL), test.t0.c0), 1)"#,
                    r#"  ├─TableReader(Build) root  data:TableFullScan"#,
                    r#"  │ └─TableFullScan cop[tikv] table:t3 keep order:false, stats:pseudo"#,
                    r#"  └─HashJoin(Probe) root  CARTESIAN left outer join, left side:TableReader"#,
                    r#"    ├─TableReader(Build) root  data:TableFullScan"#,
                    r#"    │ └─TableFullScan cop[tikv] table:t0 keep order:false, stats:pseudo"#,
                    r#"    └─Projection(Probe) root  <nil>->Column"#,
                    r#"      └─TableReader root  data:TableFullScan"#,
                    r#"        └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo"#,
                ]));
            tk.MustQuery(r#"SELECT *
FROM t0
         LEFT JOIN (SELECT NULL AS col_2
                    FROM t2) as subQuery1
                   ON true
         INNER JOIN t3 ON (((((CASE 1
                                   WHEN subQuery1.col_2 THEN t3.c0
                                   ELSE NULL END)) AND (((t0.c0))))) < 1);"#).Check(testkit::Rows("0 <nil> 3"));

            // NOT IN + NATURAL RIGHT JOIN 回归：保留完整 SQL 和结果为空的断言。
            tk.MustExec("create table chqin(id int, f1 date);");
            tk.MustExec("insert into chqin values (1,null);");
            tk.MustExec("insert into chqin values (2,null);");
            tk.MustExec("insert into chqin values (3,null);");
            tk.MustExec("create table chqin2(id int, f1 date);");
            tk.MustExec("insert into chqin2 values (1,'1990-11-27');");
            tk.MustExec("insert into chqin2 values (2,'1990-11-27');");
            tk.MustExec("insert into chqin2 values (3,'1990-11-27');");
            tk.MustQuery(r#"explain format='plan_tree' select 1 from chqin where  '2008-05-28' NOT IN
		(select a1.f1 from chqin a1 NATURAL RIGHT JOIN chqin2 a2 WHERE a2.f1  >='1990-11-27' union select f1 from chqin where id=5);"#)
                .CheckContain("Null-aware anti semi join");
            tk.MustQuery(r#"select 1 from chqin where  '2008-05-28' NOT IN
		(select a1.f1 from chqin a1 NATURAL RIGHT JOIN chqin2 a2 WHERE a2.f1  >='1990-11-27' union select f1 from chqin where id=5);"#)
                .Check(testkit::Rows());

            // Issue #66047：RIGHT OUTER JOIN ON NULL 下 FIELD 表达式仍应返回 1。
            tk.MustExec("drop table if exists t0, t1");
            tk.MustExec("CREATE TABLE t0(c0 NUMERIC UNSIGNED ZEROFILL, c1 BLOB(18), c2 NUMERIC UNSIGNED)");
            tk.MustExec("CREATE TABLE t1 LIKE t0");
            tk.MustExec("INSERT INTO t0 VALUES (0, 'wj', NULL)");
            tk.MustQuery("SELECT TRUE FROM t1 RIGHT OUTER JOIN t0 ON NULL WHERE FIELD(t0.c0, 0.0, CAST(t1.c0 AS FLOAT))")
                .Check(testkit::Rows("1"));
            tk.MustQuery("SELECT FIELD(t0.c0, 0.0, CAST(t1.c0 AS FLOAT)) FROM t1 RIGHT OUTER JOIN t0 ON NULL")
                .Check(testkit::Rows("1"));
        });

        // LATERAL selection 用例说明来自 Go 源注释，保留三类 decorrelation 行为的语义说明。
        testing::Run("TestOuter2InnerLateralSelection", || {
            tk.MustExec("drop table if exists t1, t2");
            tk.MustExec("create table t1(a1 int, b1 int)");
            tk.MustExec("create table t2(a2 int, b2 int)");
            run_outer2inner_suite_case(t, tk, cascades, "TestOuter2InnerLateralSelection");
        });

        testing::Run("TestOuter2InnerIssue55886", || {
            // 该用例依赖不同 collation_connection，Go 使用 defer 显式恢复。
            let origin_collation = tk.MustQuery("select @@collation_connection").Rows()[0][0].to_string();
            defer!(tk.MustExec("set collation_connection = ?", origin_collation));

            tk.MustExec("drop table if exists t1");
            tk.MustExec("drop table if exists t2");
            tk.MustExec("create table t1(c_foveoe text, c_jbb text, c_cz text not null)");
            tk.MustExec("create table t2(c_g7eofzlxn int)");
            tk.MustExec("set collation_connection = 'latin1_bin'");
            run_outer2inner_suite_case(t, tk, cascades, "TestOuter2InnerIssue55886");
        });
    });
}
"########;

/// 内表侧存在 null-reject 谓词时，LeftOuter 应被改写为 Inner。
#[test]
fn null_rejected_inner_predicate_converts_left_outer_join() {
    use astersql_planner_core::rule_join_reorder::{JoinNode, JoinPlan};
    use astersql_planner_core::rule_outer_to_inner_join::ConvertOuterToInnerJoin;
    use astersql_planner_core::task::JoinType;
    // 构造 LeftOuterJoin，再在其上挂 Selection，条件引用内表列且能拒绝 NULL。
    let join = crate::support::join(
        3,
        JoinType::LeftOuter,
        crate::support::leaf(1, "outer", vec![1], 10.0),
        crate::support::leaf(2, "inner", vec![2], 10.0),
        Some((1, 2)),
    );
    let selection = JoinPlan {
        id: 4,
        node: JoinNode::Selection {
            conditions: vec![crate::support::expr("gt_zero", Some(2))],
            child: Box::new(join),
        },
        schema: vec![1, 2],
        row_count: 10.0,
    };
    let (result, changed) = ConvertOuterToInnerJoin.Optimize(selection).unwrap();
    assert!(
        !changed,
        "Go ConvertOuterToInnerJoin reports planChanged=false"
    );
    // Selection 根保留，其子 Join 类型应变为 Inner。
    match result.node {
        JoinNode::Selection { child, .. } => match child.node {
            JoinNode::Join { join_type, .. } => assert_eq!(join_type, JoinType::Inner),
            _ => panic!("selection child must remain join"),
        },
        _ => panic!("selection must remain root"),
    }
}

#[test]
fn outer2inner_fixture_inventory_matches_go() {
    let cases = crate::support::fixture_case_counts(
        "outer2inner",
        &[
            "TestOuter2Inner",
            "TestOuter2InnerIssue55886",
            "TestOuter2InnerLateralSelection",
        ],
    );
    assert_eq!(cases.len(), 3);
    assert!(
        cases
            .iter()
            .all(|(input, output)| input == output && *input > 0)
    );
}

fn prepare_outer2inner_tables(test_kit: &mut TestKit) {
    test_kit.MustExec("use test", Vec::new());
    for statement in [
        "drop table if exists t, t0, t1, t2, t3, t4, t11, ti, lineitem, part, d, dd",
        "create table t1(a1 int, b1 int, c1 int)",
        "create table t2(a2 int, b2 int, c2 int)",
        "create table t3(a3 int, b3 int, c3 int)",
        "create table t4(a4 int, b4 int, c4 int)",
        "create table ti(i int)",
        "CREATE TABLE lineitem (L_PARTKEY INTEGER, L_QUANTITY DECIMAL(15,2), L_EXTENDEDPRICE DECIMAL(15,2))",
        "CREATE TABLE part(P_PARTKEY INTEGER, P_BRAND CHAR(10), P_CONTAINER CHAR(10))",
        "CREATE TABLE d (pk int, col_blob blob, col_blob_key blob, col_varchar_key varchar(1), col_date date, col_int_key int)",
        "CREATE TABLE dd (pk int, col_blob blob, col_blob_key blob, col_date date, col_int_key int)",
        "create table t0 (a0 int, b0 char, c0 char(2))",
        "create table t11 (a1 int, b1 char, c1 char)",
    ] {
        test_kit.MustExec(statement, Vec::new());
    }
}

/// Execute every Go golden case instead of merely checking that the fixture is non-empty.
#[test]
fn outer2inner_go_golden_plans_execute_in_both_planners() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = TestData::load_with_cascades(&directory, "outer2inner", true)
        .expect("load Go outer2inner fixtures");

    for cascades in [false, true] {
        let (store, _domain) = CreateMockStoreAndDomain();
        let mut test_kit = TestKit::new(store);
        prepare_outer2inner_tables(&mut test_kit);
        test_kit.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner = {}",
                if cascades { "on" } else { "off" }
            ),
            Vec::new(),
        );

        for name in [
            "TestOuter2Inner",
            "TestOuter2InnerLateralSelection",
            "TestOuter2InnerIssue55886",
        ] {
            if name == "TestOuter2InnerIssue55886" {
                test_kit.MustExec("set collation_connection = 'latin1_bin'", Vec::new());
            }
            let (input, output) = suite
                .LoadTestCasesByName(name, cascades)
                .unwrap_or_else(|error| panic!("load outer2inner/{name}: {error}"));
            let input = input
                .as_array()
                .unwrap_or_else(|| panic!("outer2inner/{name} input is not an array"));
            let output = output
                .as_array()
                .unwrap_or_else(|| panic!("outer2inner/{name} output is not an array"));
            assert_eq!(input.len(), output.len(), "outer2inner/{name} case count");

            for (index, (sql, expected)) in input.iter().zip(output).enumerate() {
                let sql = sql
                    .as_str()
                    .unwrap_or_else(|| panic!("outer2inner/{name} input case {index} is not SQL"));
                assert_eq!(
                    expected["SQL"].as_str(),
                    Some(sql),
                    "outer2inner/{name} SQL case {index}"
                );
                let expected_plan = expected["Plan"]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("outer2inner/{name} plan case {index} is not an array")
                    })
                    .iter()
                    .map(|row| {
                        row.as_str().unwrap_or_else(|| {
                            panic!("outer2inner/{name} plan row in case {index} is not a string")
                        })
                    })
                    .collect::<Vec<_>>();
                test_kit
                    .MustQuery(&format!("explain format = 'plan_tree' {sql}"), Vec::new())
                    .Check(Rows(&expected_plan));
            }
        }
    }
}

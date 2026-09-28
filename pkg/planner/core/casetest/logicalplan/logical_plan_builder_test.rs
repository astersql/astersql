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

// 逻辑计划构建器用例：schema 推导、UNION 类型提升与 year 比较回归。
//
// 对应 Go `logical_plan_builder_test.go`：用 mock store + TestKit 建表，校验
// EXISTS/DISTINCT/NATURAL RIGHT JOIN/GROUP BY 的 schema，以及 UNION 字段类型聚合
// （`AggFieldType`）与 year 列超大常数比较（issue 50235）。
//
// 逻辑计划 schema：算子输出列集合与类型；UNION 类型提升按 MySQL 规则聚合两侧类型。

// 本文件对应 pkg/planner/core/casetest/logicalplan/logical_plan_builder_test.go。
// Go 版本用 RunTestUnderCascades / CreateMockStore 验证：
//   1) EXISTS + DISTINCT + NATURAL RIGHT JOIN + GROUP BY 的 schema/plan_tree；
//   2) UNION 类型提升（int∪unsigned→longlong，literal∪unsigned bigint→decimal）；
//   3) year 列与超大常数比较（issue 50235）。
// Rust 版本同样在 classic/cascades 两种 planner 模式下运行真实 DDL/DML fixture，
// 并以 fixture 的真实列类型驱动生产 AggFieldType 核对 UNION 输出类型。

#![allow(non_snake_case)]

use astersql_parser::Parser;
use astersql_parser::ast;
use astersql_testkit::Rows;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_types::field::{AggFieldType, NewFieldType};
use astersql_types::metadata::mysql;
use std::sync::Arc;

/// 创建 mock store/domain 与绑定其上的 TestKit。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

/// 解析单条 SQL；失败则带 SQL 文本 panic。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 对齐 Go `RunTestUnderCascades`：每种 planner 模式使用独立 store/session。
fn run_test_under_cascades<F>(mut test: F)
where
    F: FnMut(&Arc<astersql_domain::Domain>, &mut TestKit, &str),
{
    for cascades in ["off", "on"] {
        let (domain, mut tk) = new_testkit();
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!("set @@tidb_enable_cascades_planner = {cascades}"),
            Vec::new(),
        );
        test(&domain, &mut tk, cascades);
    }
}

// TestGroupBySchema 对应 Go：复杂 EXISTS 子查询 schema 推导 + plan_tree。
/// 校验复合主键表元数据，以及 EXISTS 子查询 SQL 可解析且含 Where。
#[test]
fn TestGroupBySchema() {
    run_test_under_cascades(|domain, tk, cascades| {
        tk.MustExec(
            r#"CREATE TABLE mysql_3 (
    col_int_auto_increment INT(10) AUTO_INCREMENT,
    col_pk_char CHAR(60) NOT NULL,
    col_pk_date DATE NOT NULL,
    col_datetime DATETIME,
    col_int INT,
    col_date DATE,
    PRIMARY KEY (col_int_auto_increment, col_pk_char, col_datetime, col_int, col_date)
)"#,
            Vec::new(),
        );
        let table = domain
            .table_by_name("test", "mysql_3")
            .expect("mysql_3 metadata");
        assert_eq!(table.Columns.len(), 6, "planner={cascades}");
        assert!(
            table.Indices.iter().any(|idx| idx.Primary),
            "composite primary key must exist: planner={cascades}"
        );

        let sql = r#"SELECT *
FROM mysql_3 t1
WHERE EXISTS
    (SELECT DISTINCT a1.*
     FROM mysql_3 a1
     WHERE (a1.col_pk_char NOT IN
              (SELECT a1.col_pk_char
               FROM mysql_3 a1 NATURAL
               RIGHT JOIN mysql_3 a2
               WHERE t1.col_pk_date IS NULL
               GROUP BY a1.col_pk_char)) )"#;
        let plan = tk.MustQuery(
            "explain format = 'plan_tree' SELECT *
FROM mysql_3 t1
WHERE EXISTS
    (SELECT DISTINCT a1.*
     FROM mysql_3 a1
     WHERE (a1.col_pk_char NOT IN
              (SELECT a1.col_pk_char
               FROM mysql_3 a1 NATURAL
               RIGHT JOIN mysql_3 a2
               WHERE t1.col_pk_date IS NULL
               GROUP BY a1.col_pk_char)) )",
            Vec::new(),
        );
        let plan_rows = plan.Rows();
        assert!(
            !plan_rows.is_empty(),
            "EXPLAIN plan_tree must return rows: planner={cascades}"
        );

        let stmt = parse_stmt(sql);
        let select = stmt
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .expect("SelectStmt");
        assert!(
            select.Where.is_some(),
            "EXISTS predicate must be present: planner={cascades}"
        );

        // Go 期望的 plan_tree 关键：TableDual + ScalarSubQuery + Null-aware anti semi join。
        plan.CheckContain("TableDual");
        plan.CheckContain("ScalarSubQuery");
        plan.CheckContain("Null-aware anti semi join");
        plan.CheckContain("HashAgg(Build)");
        plan.CheckContain("group by:test.mysql_3.col_pk_char");
        let expected_plan = [
            ("TableDual", "root", "", "rows:0"),
            (
                "ScalarSubQuery",
                "root",
                "",
                "Output: ScalarQueryCol#29, ScalarQueryCol#30, ScalarQueryCol#31, ScalarQueryCol#32, ScalarQueryCol#33, ScalarQueryCol#34, ScalarQueryCol#35",
            ),
            (
                "└─HashJoin",
                "root",
                "",
                "Null-aware anti semi join, left side:TableReader, equal:[eq(test.mysql_3.col_pk_char, test.mysql_3.col_pk_char)]",
            ),
            (
                "  ├─HashAgg(Build)",
                "root",
                "",
                "group by:test.mysql_3.col_pk_char, funcs:firstrow(test.mysql_3.col_pk_char)->test.mysql_3.col_pk_char",
            ),
            ("  │ └─TableDual", "root", "", "rows:0"),
            ("  └─TableReader(Probe)", "root", "", "data:TableFullScan"),
            (
                "    └─TableFullScan",
                "cop[tikv]",
                "table:a1",
                "keep order:false, stats:pseudo",
            ),
        ];
        let expected_rows = expected_plan
            .into_iter()
            .map(|(id, task, access, info)| [id, task, access, info].map(str::to_owned).to_vec())
            .collect::<Vec<_>>();
        assert_eq!(plan.Rows(), expected_rows, "planner={cascades}");
    });
}

// TestLogicalPlanTypeRegression 对应 Go：year 比较 + UNION 类型提升。
/// 回归 year 超大常数比较（50235）与 UNION 类型提升（52472）。
#[test]
fn TestLogicalPlanTypeRegression() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());

    // issue:50235 — year 主键与超大无符号常数比较。
    tk.MustExec(
        "create table tt (c year(4) NOT NULL DEFAULT '2016', primary key(c))",
        Vec::new(),
    );
    tk.MustExec("insert into tt values (2016)", Vec::new());
    let tt = domain.table_by_name("test", "tt").expect("tt");
    let year_col = tt
        .Columns
        .iter()
        .find(|col| col.Name.L == "c")
        .expect("year column");
    assert_eq!(year_col.FieldType.GetType(), mysql::TypeYear);
    tk.MustQuery(
        "select /* issue:50235 */ * from tt where c < 16212511333665770580",
        Vec::new(),
    )
    .Check(Rows(&["2016"]));

    run_test_under_cascades(|domain, tk, cascades| {
        tk.MustExec("CREATE TABLE t1 ( c1 int)", Vec::new());
        tk.MustExec("CREATE TABLE t2 ( c1 int unsigned)", Vec::new());
        tk.MustExec("CREATE TABLE t3 ( c1 bigint unsigned)", Vec::new());
        tk.MustExec("INSERT INTO t1 (c1) VALUES (8)", Vec::new());
        tk.MustExec("INSERT INTO t2 (c1) VALUES (2454396638)", Vec::new());

        let t1 = domain.table_by_name("test", "t1").expect("t1 metadata");
        let t2 = domain.table_by_name("test", "t2").expect("t2 metadata");
        let t3 = domain.table_by_name("test", "t3").expect("t3 metadata");

        // issue:52472 — fixture 的 int ∪ unsigned int → longlong。
        let int_union_type = AggFieldType(&[&t1.Columns[0].FieldType, &t2.Columns[0].FieldType]);
        assert_eq!(
            mysql::TypeLonglong,
            int_union_type.GetType(),
            "SELECT c1 FROM t1 UNION ALL SELECT c1 FROM t2: planner={cascades}"
        );
        assert_eq!(
            0,
            int_union_type.GetFlag() & mysql::UnsignedFlag,
            "mixed signedness must produce a signed result: planner={cascades}"
        );

        // int literal ∪ fixture 的 unsigned bigint → decimal。
        let literal_type = NewFieldType(mysql::TypeLonglong);
        let decimal_union_type = AggFieldType(&[literal_type.as_ref(), &t3.Columns[0].FieldType]);
        assert_eq!(
            mysql::TypeNewDecimal,
            decimal_union_type.GetType(),
            "SELECT 0 UNION ALL SELECT c1 FROM t3: planner={cascades}"
        );

        let int_union = tk
            .Query("SELECT c1 FROM t1 UNION ALL SELECT c1 FROM t2", Vec::new())
            .expect("execute int union");
        assert_eq!(int_union.columns.len(), 1, "planner={cascades}");
        assert_eq!(int_union.string_rows(), Rows(&["8", "2454396638"]));

        let decimal_union = tk
            .Query("SELECT 0 UNION ALL SELECT c1 FROM t3", Vec::new())
            .expect("execute decimal union");
        assert_eq!(decimal_union.columns.len(), 1, "planner={cascades}");
        assert_eq!(decimal_union.string_rows(), Rows(&["0"]));
    });

    let _ = parse_stmt("SELECT c1 FROM t1 UNION ALL SELECT c1 FROM t2");
    let _ = parse_stmt("SELECT 0 UNION ALL SELECT c1 FROM t3");
}

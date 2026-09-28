// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 规划器历史 issue SQL 的解析与规范化回归。
//
// 覆盖 GROUP BY/HAVING、外连接空值过滤、CTE+UNION 等常见 issue 形态；
// NormalizeDigest 将字面量归一并产出 32 字节 digest，用于计划缓存键比对。

use astersql_parser::{NormalizeDigest, Parser};
use astersql_testkit::Rows;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

/// 代表性 issue 查询应能解析，且规范化后 digest 长度固定、无状态泄漏。
#[test]
fn planner_issue_queries_parse_and_normalize_without_state_leakage() {
    // HAVING / 反连接 IS NULL / CTE UNION 三类形态。
    let queries = [
        "select a, count(*) from t group by a having count(*) > 1",
        "select * from t1 left join t2 on t1.a = t2.a where t2.b is null",
        "with cte as (select a from t where a > 1) select * from cte union all select a from t",
    ];
    let mut parser = Parser::default();
    for sql in queries {
        assert!(parser.ParseOneStmt(sql, "", "").is_ok(), "{sql}");
        // NormalizeDigest：字面量归一化文本 + 固定长度指纹。
        let (normalized, digest) = NormalizeDigest(sql);
        assert!(!normalized.is_empty());
        assert_eq!(digest.Bytes().len(), 32);
    }
}

/// ONLY_FULL_GROUP_BY 场景下一元常量 `-(1)` 仍应解析为单条语句。
#[test]
fn only_full_group_by_unary_constant_keeps_one_statement() {
    let sql = "select a, -(1) from t group by a";
    let (statements, warnings) = Parser::default().Parse(sql, "", "").unwrap();
    assert_eq!(statements.len(), 1);
    assert!(warnings.is_empty());
}

/// 对应 Go `TestPlannerIssueRegressions` 中可在同一 mock domain 上执行的
/// 结果回归：保留 SQL 初始化、数据写入、排序和空/NULL 结果断言。
#[test]
fn planner_issue_execution_regressions_preserve_query_results() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());

    // null-safe join over UNION must retain the NULL=NULL match.
    tk.MustExec("create table t_base(c1 int, c2 varchar(20))", Vec::new());
    tk.MustExec("create table t_base2(c1 int, c3 int)", Vec::new());
    tk.MustExec(
        "insert into t_base values (1, 'Alice'), (NULL, 'Bob')",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_base2 values (1, 100), (NULL, NULL)",
        Vec::new(),
    );
    tk.MustQuery(
        "select base.c1, base.c2, base2.c1, base2.c3 \
         from t_base base join t_base2 base2 on base.c1 <=> base2.c1",
        Vec::new(),
    )
    .Sort()
    .Check(Rows(&["1 Alice 1 100", "<nil> Bob <nil> <nil>"]));

    // NULLIF must preserve the returned enum value type through UNION/CTE.
    tk.MustExec(
        "create table t_nullif (c6 mediumtext null, c10 enum('value1','value2','value3') null, c15 double(12,4) null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_nullif values ('sample_jNu', 'value3', 49.92)",
        Vec::new(),
    );
    tk.MustQuery(
        "select nullif(nullif(c10, c15), c15) from t_nullif",
        Vec::new(),
    )
    .Check(Rows(&["value3"]));

    // A correlated COUNT subquery must execute as a left outer join, not panic.
    tk.MustExec(
        "create table t_count1(a int primary key, b int)",
        Vec::new(),
    );
    tk.MustExec(
        "create table t_count2(a int, b int, key idx(a))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_count1 values (1, 100), (2, 200), (3, 300)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_count2 values (1, 10), (1, 20), (2, 30), (4, 40)",
        Vec::new(),
    );
    tk.MustQuery(
        "select t_count1.b, (select count(*) from t_count2 where t_count2.a=t_count1.a) \
         from t_count1 where t_count1.a=1",
        Vec::new(),
    )
    .Check(Rows(&["100 2"]));

    // Constant GROUP BY/HAVING must not manufacture rows from NULL values.
    tk.MustExec("create table t_group(c0 int)", Vec::new());
    tk.MustExec("insert into t_group values (1), (0), (NULL)", Vec::new());
    tk.MustQuery(
        "select c0 from t_group group by null having c0 order by c0",
        Vec::new(),
    )
    .Check(Rows(&["1"]));
    tk.MustQuery(
        "select c0 from t_group group by null having not(c0) order by c0",
        Vec::new(),
    )
    .Check(Rows(&[]));

    // The equality-expression/view regression must complete with an empty result.
    tk.MustExec("create table t_view(c2 tinyint)", Vec::new());
    tk.MustExec(
        "create view v_view(c0) as select false from t_view",
        Vec::new(),
    );
    tk.MustQuery(
        "select 1 from t_view, v_view where t_view.c2=(-(-1|v_view.c0))",
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// 继续覆盖 Go 回归中的错误分类、用户变量计划和分区 NULL-safe 比较。
#[test]
fn planner_issue_boundary_regressions_keep_errors_and_plan_shape() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());

    // Point-update fast path must not wrap a negative value into UNSIGNED.
    tk.MustExec(
        "create table t_unsigned (id int primary key, bing bigint unsigned default null)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_unsigned values (1, 1), (2, null)",
        Vec::new(),
    );
    for sql in [
        "update t_unsigned set bing = (select -1) where id = 2",
        "update t_unsigned set bing = -1 where id = 2",
    ] {
        let error = tk.ExecToErr(sql);
        assert!(
            error
                .message()
                .to_ascii_lowercase()
                .contains("out of range"),
            "{sql}: unexpected error: {error}"
        );
    }

    // Upper-case read-only user variables must be constant-folded like lower-case
    // names, leaving the indexed range access available to the planner.
    tk.MustExec("create table t_var (a int, key(a))", Vec::new());
    tk.MustExec("set @a=1", Vec::new());
    let lower_plan = tk.MustQuery("explain select a from t_var where a=@a", Vec::new());
    assert!(
        lower_plan
            .Rows()
            .iter()
            .flatten()
            .any(|cell| cell.contains("IndexRangeScan")),
        "lower-case variable plan: {:?}",
        lower_plan.Rows()
    );
    tk.MustExec("set @A=1", Vec::new());
    let upper_plan = tk.MustQuery("explain select a from t_var where a=@A", Vec::new());
    assert!(
        upper_plan
            .Rows()
            .iter()
            .flatten()
            .any(|cell| cell.contains("IndexRangeScan")),
        "upper-case variable plan: {:?}",
        upper_plan.Rows()
    );

    // NULL-safe equality with a constant must prune only the non-matching
    // partition and retain the NULL row.
    tk.MustExec(
        "create table t_range (a int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than maxvalue)",
        Vec::new(),
    );
    tk.MustExec("insert into t_range values (1), (11), (null)", Vec::new());
    tk.MustQuery("select a from t_range where 1 <=> a", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("select a from t_range where null <=> a", Vec::new())
        .Check(Rows(&["<nil>"]));

    // SIGN on a view predicate must not leak decimal scale or warnings.
    tk.MustExec("create table t_sign0(c0 numeric)", Vec::new());
    tk.MustExec("create table t_sign1 like t_sign0", Vec::new());
    tk.MustExec("replace into t_sign0 values (-1780864408)", Vec::new());
    tk.MustExec("insert into t_sign1 values (1448472626)", Vec::new());
    tk.MustExec(
        "create or replace view v_sign(c0) as select 0.99 from t_sign1, t_sign0",
        Vec::new(),
    );
    let sign_rows = tk
        .MustQuery(
            "select v_sign.c0 from v_sign where sign(v_sign.c0)",
            Vec::new(),
        )
        .Rows();
    assert_eq!(sign_rows, Rows(&["0.99"]), "SIGN view result");
    let warning_rows = tk.MustQuery("show warnings", Vec::new()).Rows();
    assert_eq!(warning_rows, Rows(&[]), "SIGN view warnings");
}

/// 覆盖 Go 回归中此前遗漏的 SQL mode、子查询与写路径错误契约。
#[test]
fn planner_issue_error_and_rewrite_regressions_match_go_contracts() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());

    // ONLY_FULL_GROUP_BY 对基表与由其创建的视图必须给出相同错误；关闭
    // sql_mode 后两者都应恢复可执行。
    tk.MustExec("create table t_ofgb(a int)", Vec::new());
    // Rust mock session does not inherit TiDB's process-level default mode, so
    // select the same mode explicitly before exercising the planner contract.
    tk.MustExec("set @@sql_mode = 'ONLY_FULL_GROUP_BY'", Vec::new());
    let table_result = tk.Exec("select * from t_ofgb group by null", Vec::new());
    assert!(
        table_result.is_err(),
        "base-table query unexpectedly succeeded"
    );
    let table_error = table_result.unwrap_err();
    assert!(
        table_error.message().contains("not in GROUP BY clause"),
        "unexpected ONLY_FULL_GROUP_BY error: {table_error}"
    );
    tk.MustExec(
        "create view v_ofgb as select * from t_ofgb group by null",
        Vec::new(),
    );
    let view_result = tk.Exec("select * from v_ofgb", Vec::new());
    assert!(view_result.is_err(), "view query unexpectedly succeeded");
    let view_error = view_result.unwrap_err();
    assert!(
        view_error.message().contains("not in GROUP BY clause"),
        "unexpected view error: {view_error}"
    );
    tk.MustExec("set @@sql_mode = ''", Vec::new());
    tk.MustQuery("select * from t_ofgb group by null", Vec::new())
        .Check(Rows(&[]));
    tk.MustQuery("select * from v_ofgb", Vec::new())
        .Check(Rows(&[]));

    // NULL 比较被化简为 TableDual 时，UnionScan 不得重新制造行。
    tk.MustExec("create table t_union(a int)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t_union values (1)", Vec::new());
    tk.MustQuery("select * from t_union where a = null", Vec::new())
        .Check(Rows(&[]));
    tk.MustExec("rollback", Vec::new());
}

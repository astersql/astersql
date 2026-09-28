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

// 二级 panic 风险回归：参数化输入与 DISTINCT 聚合 / 递归 CTE。
//
// 历史问题中，零参日期函数在参数化路径可能 panic；DISTINCT 聚合与递归 CTE
//（Common Table Expression，公用表表达式）序列也曾在 AST 构建阶段失败。

use astersql_parser::Parser;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

/// 零参日期/时间格式化函数在 ParseOneStmt 路径上不得 panic。
#[test]
fn zero_argument_date_functions_never_panic_during_parameterization_input_parse() {
    // 用 catch_unwind 捕获解析期 panic，确保参数化输入边界安全。
    for function in [
        "date_format()",
        "str_to_date()",
        "time_format()",
        "from_unixtime()",
    ] {
        let sql = format!("select {function} from t");
        let result = std::panic::catch_unwind(|| Parser::default().ParseOneStmt(&sql, "", ""));
        assert!(result.is_ok(), "{sql} panicked");
    }
}

/// 对应 Go `TestNonPreparedPlanCacheZeroArgDateFunc`：必须经过真实会话的
/// 非预编译计划缓存参数化路径，而不是只验证 parser AST。
#[test]
fn non_prepared_plan_cache_zero_argument_date_functions_do_not_panic() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set tidb_enable_non_prepared_plan_cache=1", Vec::new());
    tk.MustExec("create table t (a datetime, b int)", Vec::new());
    tk.MustExec(
        "insert into t values ('2020-01-01 00:00:00', 1)",
        Vec::new(),
    );

    for sql in [
        "select * from t where a = date_format()",
        "select * from t where a = str_to_date()",
        "select * from t where b = time_format()",
        "select * from t where b = from_unixtime()",
    ] {
        let result = tk.Exec(sql, Vec::new());
        assert!(
            result.is_err(),
            "invalid zero-argument call unexpectedly succeeded: {sql}"
        );
    }
}

/// DISTINCT 聚合与递归 CTE 应能解析为真实 AST（而非桩/空结果）。
#[test]
fn distinct_aggregate_and_cte_sequence_regressions_parse_as_real_ast() {
    for sql in [
        "select count(distinct 1), sum(distinct a) from t",
        "with recursive cte(n) as (select 1 union all select n + 1 from cte where n < 3) select * from cte",
    ] {
        assert!(Parser::default().ParseOneStmt(sql, "", "").is_ok(), "{sql}");
    }
}

/// 对应 Go `TestSkewDistinctAggConstantArg`：常量 DISTINCT 参数不能被错误
/// 地当作列节点，真实聚合结果必须保持每个分组一行。
#[test]
fn skew_distinct_aggregate_constant_arguments_keep_values() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set tidb_opt_skew_distinct_agg=1", Vec::new());
    tk.MustExec("create table t (a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 10), (2, 10), (3, 20)", Vec::new());
    tk.MustQuery(
        "select b, count(distinct 1) from t group by b order by b",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&["10 1", "20 1"]));
    tk.MustQuery(
        "select b, sum(distinct 2) from t group by b order by b",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&["10 2", "20 2"]));
}

/// 对应 Go `TestPushDownSequenceWithTableDual`：共享 CTE 被常量假条件
/// 消除为无子节点 TableDual 时，sequence 下推仍须返回空集而不能索引子节点。
#[test]
fn push_down_sequence_with_table_dual_returns_empty_result() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set tidb_opt_enable_mpp_shared_cte_execution=1", Vec::new());
    tk.MustExec("create table t (a int)", Vec::new());
    tk.MustExec("insert into t values (1), (2)", Vec::new());
    tk.MustQuery(
        "with cte as (select a from t) \
         select * from cte c1 join cte c2 on c1.a = c2.a where 1 = 0",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&[]));
}

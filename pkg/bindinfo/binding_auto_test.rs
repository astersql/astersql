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

// bindinfo 自动绑定（auto binding）测试模块。
//
// 本文件是从 Go(TiDB) 的 `binding_auto_test.go` 机械迁移而来的测试基线，
// 覆盖 SQL 绑定（SQL Binding，即把某条 SQL 固定到指定执行计划上的机制）
// 的自动探索能力，主要包括：
// - `GenBriefPlanWithSCtx`：在会话上下文中为语句生成简要执行计划
//   （执行计划指优化器为 SQL 选择的具体执行方式，如索引扫描、聚合算子等）；
// - `EXPLAIN EXPLORE` 语句：枚举同一条 SQL 的多个候选计划（含不同索引提示、
//   `no_decorrelate` 去关联提示等），供用户挑选并生成绑定；
// - `IsSimplePointPlan`：判断计划文本是否为简单点查
//   （Point Get，按主键或唯一索引直接定位单行的最快访问路径）；
// - `RecordRelevantOptVarsAndFixes`：记录影响计划选择的优化器变量与 fix 项。
//
// 文件结构说明：大部分 Go 测试逻辑以原始字符串常量的形式整体保留为
// 迁移草稿（不参与编译执行），文件末尾另有一个可实际运行的 Rust 冒烟测试。

/// Go 原测试文件的机械迁移草稿，以原始字符串字面量整体保存。
///
/// 内容对应 Go 的 `TestGenPlanWithSCtx`、`TestExplainExploreBasic`、
/// `TestExplainExploreIndexHints`、`TestExplainExploreIndexHintWithAlias`、
/// `TestExplainExploreNoDecorrelateHint`、`TestIsSimplePointPlan`、
/// `TestRelevantOptVarsAndFixes`、`TestRelevantOptVarsCorrelateSubquery`、
/// `TestExplainExploreAnalyze`、`TestExplainExploreVerifyAndBind`、
/// `TestPlanGeneration` 等用例，保留了原有的测试步骤、SQL 与断言顺序，
/// 作为后续将其逐个改写为可执行 Rust 测试的参考基线。
/// 该常量仅承载文本，不会被编译为测试逻辑。
const GO_BINDING_AUTO_TEST_DRAFT: &str = r########################################"
type Error = Box<dyn std::error::Error>;
pub struct TestingT;
pub struct BenchmarkDraft;
macro_rules! defer_draft { ($($tt:tt)*) => {}; }
macro_rules! spawn_go_draft { ($($tt:tt)*) => {}; }

// test_gen_plan_with_s_ctx 对应 Go 的 TestGenPlanWithSCtx，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_gen_plan_with_s_ctx() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t1 (a int, b int, c int, key(a), key(b))`);
        tk::MustExec(`create table t2 (a int, b int, c int, key(a), key(b))`);

        let mut p = parser::New();
        let mut sctx = tk::Session();
        sctx::GetSessionVars().CostModelVersion = 2;
        let mut check = func(sql, expectedHint, expectedPlan string) {
            p::Reset();
            let mut stmt, err = p::ParseOneStmt(sql, "", "");
            require::NoErrorf(t, err, "sql: %s", sql);
            let mut planDigest, planHint, planText, err = bindinfo::GenBriefPlanWithSCtx(sctx, stmt);
            require::NoErrorf(t, err, "sql: %s", sql);
            require::Greaterf(t, len(planDigest), 0, "sql: %s", sql);
            require::Truef(t, strings::Contains(planHint, expectedHint), "sql: %s", sql);
            let mut planOperators = make([]string, 0, len(planText));
            let mut for _, row = range planText {
                planOperators = append(planOperators, row[0]);
            }
            require::Truef(t, strings::Contains(strings::Join(planOperators, ","), expectedPlan), "sql: %s", sql);
        }
        check("select count(1) from t1 where a=1",
            "stream_agg", "StreamAgg");

        sctx::GetSessionVars().StreamAggCostFactor = 10000;
        check("select count(1) from t1 where a=1",
            "hash_agg", "HashAgg");
        sctx::GetSessionVars().StreamAggCostFactor = 1;

        check("select * from t1, t2 where t1.a=t2.a and t2.b=1",
            "inl_hash_join", "IndexHashJoin");

        sctx::GetSessionVars().IndexJoinCostFactor = 100000;
        sctx::GetSessionVars().HashJoinCostFactor = 100000;
        check("select * from t1, t2 where t1.a=t2.a and t2.b=1",
            "merge_join", `MergeJoin`);
}

// test_explain_explore_basic 对应 Go 的 TestExplainExploreBasic，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_explain_explore_basic() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");

        let mut check = func(sql string, expectedRowCount int) {
            let mut rows = tk::MustQuery(sql).Rows();
            require::Equalf(t, expectedRowCount, len(rows), "sql: %s", sql);
            let mut for _, row = range rows {
                let mut planDigest = row[3];
                require::NotEmptyf(t, planDigest, "sql: %s", sql);
            }
        }

        tk::MustExec(`create table t (a int, b int, c varchar(10), key(a))`);
        check(`explain explore select a from t where b=1`, 1);
        tk::MustExec(`create global binding using select a from t where b=1`);
        check(`explain explore select a from t where b=1`, 2);
        check(`explain explore SELECT a FROM t WHERE b=1`, 2);
        check(`explain explore SELECT a FROM t WHERE b= 1`, 2);
        check(`explain explore      SELECT  a FROM test.t WHERE b= 1`, 2);
        require::GreaterOrEqual(t, len(tk::MustQuery(`explain explore "23109784b802bcef5398dd81d3b1c5b79200c257c101a5b9f90758206f3d09ed"`).Rows()), 1);

        check(`explain explore select a from t where b in (1, 2, 3)`, 1);
        tk::MustExec(`create global binding using select a from t where b in (1, 2, 3)`);
        check(`explain explore select a from t where b in (1, 2, 3)`, 2);
        check(`explain explore select a from t where b in (1, 2)`, 2);
        check(`explain explore select a from t where b in (1)`, 2);
        check(`explain explore SELECT a from t WHere b in (1)`, 2);

        check(`explain explore select a from t where c = ''`, 1);
        tk::MustExec(`create global binding using select a from t where c = ''`);
        check(`explain explore select a from t where c = ''`, 2);
        check(`explain explore select a from t where c = '123'`, 2);
        check(`explain explore select a from t where c = '\"'`, 2);
        check(`explain explore select a from t where c = '              '`, 2);
        check(`explain explore select a from t where c = ""`, 2);
        check(`explain explore select a from t where c = "\'"`, 2);

        tk::MustExecToErr("explain explore 'xxx'", "");
        tk::MustExecToErr("explain explore SELECT A FROM", "");
}

// test_explain_explore_index_hints 对应 Go 的 TestExplainExploreIndexHints，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_explain_explore_index_hints() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t (a int, b int, c int, key(a), key(b))`);

        let mut rows = tk::MustQuery(`explain explore select * from t where a=1 and b=1`).Rows();
        let mut hasIndexA, hasIndexB = false, false;
        let mut for _, row = range rows {
            let mut plan = row[2].(string);
            if strings::Contains(plan, "index:a") {
                hasIndexA = true;
            }
            if strings::Contains(plan, "index:b") {
                hasIndexB = true;
            }
        }
        require::True(t, hasIndexA, "expected index a plan in explain explore output");
        require::True(t, hasIndexB, "expected index b plan in explain explore output");
}

// test_explain_explore_index_hint_with_alias 对应 Go 的 TestExplainExploreIndexHintWithAlias，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_explain_explore_index_hint_with_alias() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t (a int, b int, x varchar(10), key(a), key(b))`);

        let mut rows = tk::MustQuery(`explain explore select 1 from t t_alias where a=1 and b=1 and x like "%xx%"`).Rows();
        let mut hasIndexA, hasIndexB = false, false;
        let mut for _, row = range rows {
            let mut plan = row[2].(string);
            if strings::Contains(plan, "index:a") {
                hasIndexA = true;
            }
            if strings::Contains(plan, "index:b") {
                hasIndexB = true;
            }
        }
        require::True(t, hasIndexA, "expected index a plan in explain explore output");
        require::True(t, hasIndexB, "expected index b plan in explain explore output");
}

// test_explain_explore_no_decorrelate_hint 对应 Go 的 TestExplainExploreNoDecorrelateHint，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_explain_explore_no_decorrelate_hint() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table o (a int, b int, c int, d int, key(b))`);
        tk::MustExec(`create table r (a int, b int, key(a), key(b))`);
        tk::MustExec(`create table o1 (a int, key(a))`);

        let mut rows = tk::MustQuery(`explain explore select o.* from o where exists (select 1 from r inner join o1 on o1.a=r.a where r.b=o.b)`).Rows();
        let mut hasNoDecorrelate = false;
        let mut for _, row = range rows {
            if strings::Contains(row[1].(string), "no_decorrelate") {
                hasNoDecorrelate = true;
                break;
            }
        }
        require::True(t, hasNoDecorrelate, "expected no_decorrelate plan in explain explore output");
}

// test_is_simple_point_plan 对应 Go 的 TestIsSimplePointPlan，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_is_simple_point_plan() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        require::True(t, bindinfo::IsSimplePointPlan(`       id  task    estRows operator info  actRows execution info  memory          disk;
            Projection_4    root    1       plus(test.t.a, 1)->Column#3     0       time:173µs, open:24.9µs, close:8.92µs, loops:1, Concurrency:OFF                         380 Bytes       N/A;
            └─Point_Get_5   root    1       table:t, handle:2               0       time:143.2µs, open:1.71µs, close:5.92µs, loops:1, Get:{num_rpc:1, total_time:40µs}      N/A             N/A`));
        require::True(t, bindinfo::IsSimplePointPlan(`       id  task    estRows operator info  actRows execution info  memory          disk;
            Point_Get_5   root    1       table:t, handle:2               0       time:143.2µs, open:1.71µs, close:5.92µs, loops:1, Get:{num_rpc:1, total_time:40µs}      N/A             N/A`));
        require::True(t, bindinfo::IsSimplePointPlan(`Point_Get_5   root    1       table:t, handle:2               0       time:143.2µs, open:1.71µs, close:5.92µs, loops:1, Get:{num_rpc:1, total_time:40µs}      N/A             N/A`));
        require::True(t, bindinfo::IsSimplePointPlan(`id                      task    estRows operator info                                           actRows execution info                                                                                                                    memory          disk;
            Projection_4            root    3.00    plus(test.t.a, 1)->Column#3                             0       time:218.3µs, open:14.5µs, close:9.79µs, loops:1, Concurrency:OFF                                                                 145 Bytes       N/A;
            └─Batch_Point_Get_5     root    3.00    table:t, handle:[1 2 3], keep order:false, desc:false   0       time:201.1µs, open:3.83µs, close:6.46µs, loops:1, BatchGet:{num_rpc:2, total_time:65.7µs}, rpc_errors:{epoch_not_match:1} N/A             N/A   `));
        require::True(t, bindinfo::IsSimplePointPlan(`id                      task    estRows operator info                                           actRows execution info                                                                                                                    memory          disk;
            Batch_Point_Get_5     root    3.00    table:t, handle:[1 2 3], keep order:false, desc:false   0       time:201.1µs, open:3.83µs, close:6.46µs, loops:1, BatchGet:{num_rpc:2, total_time:65.7µs}, rpc_errors:{epoch_not_match:1} N/A             N/A   `));
        require::True(t, bindinfo::IsSimplePointPlan(`id                      task    estRows operator info                                           actRows execution info                                                                                                                    memory          disk;
            Selection ....;
            └─Batch_Point_Get_5     root    3.00    table:t, handle:[1 2 3], keep order:false, desc:false   0       time:201.1µs, open:3.83µs, close:6.46µs, loops:1, BatchGet:{num_rpc:2, total_time:65.7µs}, rpc_errors:{epoch_not_match:1} N/A             N/A   `));

        require::False(t, bindinfo::IsSimplePointPlan(`       id                      task            estRows operator info                           actRows execution info memory          disk;
            TableReader_5           root            10000   data:TableFullScan_4                    0       time:456.3µs, open:141µs, close:6.79µs, loops:1, cop_task: {num: 1, max: 241.3µs, proc_keys: 0, copr_cache_hit_ratio: 0.00, build_task_duration: 91.5µs, max_distsql_concurrency: 1}, rpc_info:{Cop:{num_rpc:1, total_time:203.9µs}}      182 Bytes       N/A;
            └─TableFullScan_4       cop[tikv]       10000   table:t, keep order:false, stats:pseudo 0       tikv_task:{time:155.2µs, loops:0}                                                                                                                                                                                                         N/A             N/A `));
        require::False(t, bindinfo::IsSimplePointPlan(`id                      task    estRows operator info                                           actRows execution info                                                                                                                    memory          disk;
            HashAgg            root    3.00    plus(test.t.a, 1)->Column#3                             0       time:218.3µs, open:14.5µs, close:9.79µs, loops:1, Concurrency:OFF                                                                 145 Bytes       N/A;
            └─Batch_Point_Get_5     root    3.00    table:t, handle:[1 2 3], keep order:false, desc:false   0       time:201.1µs, open:3.83µs, close:6.46µs, loops:1, BatchGet:{num_rpc:2, total_time:65.7µs}, rpc_errors:{epoch_not_match:1} N/A             N/A   `));
        require::False(t, bindinfo::IsSimplePointPlan(`       id  task    estRows operator info  actRows execution info  memory          disk;
            HashJoin    root    1       plus(test.t.a, 1)->Column#3     0       time:173µs, open:24.9µs, close:8.92µs, loops:1, Concurrency:OFF                         380 Bytes       N/A;
            └─Point_Get_5   root    1       table:t, handle:2               0       time:143.2µs, open:1.71µs, close:5.92µs, loops:1, Get:{num_rpc:1, total_time:40µs}      N/A             N/A`));
        require::False(t, bindinfo::IsSimplePointPlan(``));
        require::False(t, bindinfo::IsSimplePointPlan(`  \n   `));
}

// test_relevant_opt_vars_and_fixes 对应 Go 的 TestRelevantOptVarsAndFixes，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_relevant_opt_vars_and_fixes() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t1 (a int, b int, c varchar(10), key(a), key(b))`);
        tk::MustExec(`create table t2 (a int, b int, c varchar(10), key(a), key(b))`);

        let mut input: Vec<String>;
        let mut output: Vec<struct {>;
            Vars  string;
            Fixes string;
        }
        // testdata 录制逻辑保持 Go 黄金文件语义，不写回数据。
    bindingAutoSuiteData::LoadTestCases(t, &input, &output);
        let mut p = parser::New();
        let mut for i, sql = range input {
            p::Reset();
            let mut stmt, err = p::ParseOneStmt(sql, "", "");
            require::NoErrorf(t, err, "sql: %s", sql);
            let mut vars, fixes, err = bindinfo::RecordRelevantOptVarsAndFixes(tk::Session(), stmt);
            require::NoErrorf(t, err, "sql: %s", sql);
            // testdata 录制逻辑保持 Go 黄金文件语义，不写回数据。
        testdata::OnRecord(func() {;
                output[i].Vars = fmt::Sprintf("%v", vars);
                output[i].Fixes = fmt::Sprintf("%v", fixes);
            });
            require::Equalf(t, fmt::Sprintf("%v", vars), output[i].Vars, "sql: %s", sql);
            require::Equalf(t, fmt::Sprintf("%v", fixes), output[i].Fixes, "sql: %s", sql);
        }
}

// test_relevant_opt_vars_correlate_subquery 对应 Go 的 TestRelevantOptVarsCorrelateSubquery，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_relevant_opt_vars_correlate_subquery() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t1 (a int, b int, key(a))`);
        tk::MustExec(`create table t2 (a int, b int, key(a))`);

        let mut p = parser::New();
        let mut sql = "select * from t1 where a in (select a from t2)";

        // The alternative logical plans variable is recorded as relevant because the
        // code path where it affects plan choice (correlate-to-Apply) was reached.
        let mut for _, enabled = range []string{"OFF", "ON"} {
            tk::MustExec("set tidb_opt_enable_alternative_logical_plans = " + enabled);
            p::Reset();
            let mut stmt, err = p::ParseOneStmt(sql, "", "");
            require::NoError(t, err);
            let mut vars, _, err = bindinfo::RecordRelevantOptVarsAndFixes(tk::Session(), stmt);
            require::NoError(t, err);
            require::True(t, slices::Contains(vars, vardef.TiDBOptEnableAlternativeLogicalPlans),
                "enabled=%s: expected %s in recorded vars %v", enabled, vardef.TiDBOptEnableAlternativeLogicalPlans, vars);
        }
}

// test_explain_explore_analyze 对应 Go 的 TestExplainExploreAnalyze，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_explain_explore_analyze() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t (a int, b int, key(a))`);
        tk::MustExec(`insert into t values (1, 2), (2, 3), (3, 4), (4, 5)`);

        let mut checkExecInfo = func(sql string, hasExecInfo bool) {
            let mut rs = tk::MustQuery(sql).Rows();
            let mut for _, row = range rs {
                let mut latency = row[4].(string);
                let mut execTimes = row[5].(string);
                let mut retRows = row[7].(string);
                if !hasExecInfo {
                    require::Equalf(t, "0", latency, "sql: %s", sql);
                    require::Equalf(t, "0", execTimes, "sql: %s", sql);
                    require::Equalf(t, "0", retRows, "sql: %s", sql);
                } else {
                    require::NotEqualf(t, "0", latency, "sql: %s", sql);
                    require::NotEqualf(t, "0", execTimes, "sql: %s", sql);
                    require::NotEqualf(t, "0", retRows, "sql: %s", sql);
                }
            }
        }

        checkExecInfo(`explain explore select * from t where a=1`, false);
        checkExecInfo(`explain explore analyze select * from t where a=1`, true);
        checkExecInfo(`explain explore select * from t where b<10`, false);
        checkExecInfo(`explain explore analyze select * from t where b<10`, true);
        checkExecInfo(`explain explore select count(1) from t where b<10`, false);
        checkExecInfo(`explain explore analyze select count(1) from t where b<10`, true);
}

// test_explain_explore_verify_and_bind 对应 Go 的 TestExplainExploreVerifyAndBind，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_explain_explore_verify_and_bind() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        require::NoError(t, tk::Session().Auth(auth.UserIdentity {Username: "root", Hostname: "%"}, None, None, None));
        tk::MustExec("use test");
        tk::MustExec(`create table t (a int, b int, key(a))`);
        tk::MustExec(`insert into t values (1, 2), (2, 3), (3, 4), (4, 5)`);

        tk::MustQuery(`select * from t`);
        tk::MustQuery(`select @@last_plan_from_binding`).Check(testkit::Rows("0"));
        require::Equal(t, 0, len(tk::MustQuery(`show global bindings`).Rows())) // no binding;

        let mut rs = tk::MustQuery(`explain explore select * from t`).Rows();
        let mut runStmt = rs[0][12].(string)    // "EXPLAIN ANALYZE <bind_sql>";
        let mut bindingSQL = rs[0][13].(string) // "CREATE GLOBAL BINDING USING <bind_sql>";

        require::True(t, strings::HasPrefix(runStmt, "EXPLAIN ANALYZE"));
        require::True(t, strings::HasPrefix(bindingSQL, "CREATE GLOBAL BINDING USING"));

        rs = tk::MustQuery(runStmt).Rows();
        require::True(t, strings::Contains(rs[0][0].(string), "TableReader")) // table scan and no error;

        tk::MustExec(bindingSQL);
        tk::MustQuery(`select * from t`);
        tk::MustQuery(`select @@last_plan_from_binding`).Check(testkit::Rows("1"));
        require::Equal(t, 1, len(tk::MustQuery(`show global bindings`).Rows()));
}

// test_plan_generation 对应 Go 的 TestPlanGeneration，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_plan_generation() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t (a int, b int, c int, key(a))`);
        tk::MustExec(`create table t1 (a int, b int, c int, key(a), key(b))`);
        tk::MustExec(`create table t2 (a int, b int, c int, key(a), key(b))`);
        tk::MustExec(`create table t3 (a int, b int, c int, key(a), key(b))`);

        let mut input: Vec<String>;
        let mut output: Vec<struct {>;
            SQL  string;
            Plan [][]string;
        }
        // testdata 录制逻辑保持 Go 黄金文件语义，不写回数据。
    bindingAutoSuiteData::LoadTestCases(t, &input, &output);
        let mut for i, sql = range input {
            let mut rows = tk::MustQuery(sql).Rows();
            let mut for rowID, row = range rows {
                let mut plan = strings::Split(strings::Replace(row[2].(string), "\t", "  ", -1), "\n");
                // testdata 录制逻辑保持 Go 黄金文件语义，不写回数据。
            testdata::OnRecord(func() {;
                    output[i].SQL = sql;
                    if len(output[i].Plan) < rowID {
                        output[i].Plan[rowID] = plan;
                    } else {
                        output[i].Plan = append(output[i].Plan, plan);
                    }
                });
                require::Equalf(t, plan, output[i].Plan[rowID], "sql: %s", sql);
            }
        }
}
"########################################;

/// 冒烟测试：验证 `IsSimplePointPlan` 对典型计划文本的正反例分类。
///
/// - 正例：`Point_Get`（点查，按主键/唯一索引直接读取单行）与
///   `Batch_Point_Get`（批量点查，一次读取多个主键对应的行）应被判定为简单点查计划；
/// - 反例：`IndexRangeScan`（索引范围扫描，需要遍历一段索引区间）不属于点查，
///   应返回 false。
///
/// 该判断用于自动绑定流程：简单点查计划通常已是最优，无需再为其探索候选计划。
#[test]
fn canonical_simple_point_plan_classification_covers_positive_and_negative_plans() {
    assert!(crate::IsSimplePointPlan("Point_Get table:t"));
    assert!(crate::IsSimplePointPlan("Batch_Point_Get table:t"));
    assert!(!crate::IsSimplePointPlan("IndexRangeScan table:t"));
}

use crate::{
    BindError, Binding, BindingPlanEvolution, BindingPlanInfo, ExploreContext, GenerationSpec,
    PlanExecInfo, PlanGenerator, PlanPerfPredictor, PlanRuntime, Result, binding_auto::bindingAuto,
    genedPlan, state,
};
use std::sync::{Arc, Mutex};

struct FixedGenerator(Vec<BindingPlanInfo>);

impl PlanGenerator for FixedGenerator {
    fn Generate(&self, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<BindingPlanInfo>> {
        Ok(self.0.clone())
    }
}

struct ZeroPredictor;

impl PlanPerfPredictor for ZeroPredictor {
    fn PerfPredicate(&self, plans: &mut [BindingPlanInfo]) -> Result<(Vec<f64>, Vec<String>)> {
        Ok((vec![0.0; plans.len()], vec![String::new(); plans.len()]))
    }
}

struct RecordingRuntime {
    historical: Vec<Arc<Binding>>,
    executions: Mutex<Vec<String>>,
    historical_info: Option<PlanExecInfo>,
}

impl PlanRuntime for RecordingRuntime {
    fn historical_bindings(&self, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<Arc<Binding>>> {
        Ok(self.historical.clone())
    }

    fn plan_exec_info(&self, _: &str) -> Result<Option<PlanExecInfo>> {
        Ok(self.historical_info.clone())
    }

    fn execute_binding(&self, binding: &Binding) -> Result<PlanExecInfo> {
        self.executions
            .lock()
            .unwrap()
            .push(binding.BindSQL.clone());
        Ok(PlanExecInfo {
            ExecCount: 1,
            ResultRows: 1,
            ..PlanExecInfo::default()
        })
    }

    fn generation_spec(&self, _: &str, _: &str, _: &str, _: &str) -> Result<GenerationSpec> {
        Err(BindError("unused generation_spec".to_owned()))
    }

    fn plan_under_state(&self, _: &GenerationSpec, _: &state) -> Result<genedPlan> {
        Err(BindError("unused plan_under_state".to_owned()))
    }
}

fn candidate(sql: &str, digest: &str) -> BindingPlanInfo {
    BindingPlanInfo {
        Binding: Arc::new(Binding {
            BindSQL: sql.to_owned(),
            PlanDigest: digest.to_owned(),
            ..Binding::default()
        }),
        ..BindingPlanInfo::default()
    }
}

#[test]
fn analyze_executes_only_generated_candidates_like_go() {
    let runtime = Arc::new(RecordingRuntime {
        historical: vec![candidate("historical", "history-digest").Binding],
        executions: Mutex::new(Vec::new()),
        historical_info: None,
    });
    let evolution = bindingAuto {
        runtime: runtime.clone(),
        planGenerator: Box::new(FixedGenerator(vec![candidate(
            "generated",
            "generated-digest",
        )])),
        ruleBasedPredictor: Box::new(ZeroPredictor),
        llmPredictor: Box::new(ZeroPredictor),
    };

    evolution
        .ExplorePlansForSQL(&ExploreContext::default(), "select 1", true)
        .unwrap();

    assert_eq!(*runtime.executions.lock().unwrap(), vec!["generated"]);
}

#[test]
fn non_positive_historical_exec_count_leaves_candidate_stats_empty_like_go() {
    let runtime = Arc::new(RecordingRuntime {
        historical: vec![candidate("historical", "history-digest").Binding],
        executions: Mutex::new(Vec::new()),
        historical_info: Some(PlanExecInfo {
            Plan: "stale plan".to_owned(),
            ExecCount: 0,
            ResultRows: 99,
            ProcessedKeys: 88,
            TotalTime: 77,
        }),
    });
    let evolution = bindingAuto {
        runtime,
        planGenerator: Box::new(FixedGenerator(Vec::new())),
        ruleBasedPredictor: Box::new(ZeroPredictor),
        llmPredictor: Box::new(ZeroPredictor),
    };

    let plans = evolution
        .ExplorePlansForSQL(&ExploreContext::default(), "select 1", false)
        .unwrap();

    assert_eq!(plans.len(), 1);
    assert!(plans[0].Plan.is_empty());
    assert_eq!(plans[0].ExecTimes, 0);
    assert_eq!(plans[0].AvgReturnedRows, 0.0);
}

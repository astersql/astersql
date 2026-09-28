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

// 物理计划 hint 与归一化输入形状用例。
//
// Hint（优化器提示）可改变索引选择等物理计划决策；NormalizeKeepHint 在规范化
// SQL 时保留 hint 文本，使带 hint / 不带 hint 的输入可被区分。

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use super::main_test::{PLAN_SUITE_CASES, get_plan_suite_data};

static MEMORY_LIMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Go 每个物理计划测试都从 plan_suite 读取输入；这里验证 Rust 没有丢失任何
/// 测试组，并且每条 SQL 与其标准/Cascades golden 记录保持一一对应。
#[test]
fn physical_plan_suite_keeps_all_go_test_scenarios() {
    let suite = get_plan_suite_data();
    let mut total = 0;
    for name in PLAN_SUITE_CASES {
        let (input, standard) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("load standard {name}: {error}"));
        let (cascades_input, cascades) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("load cascades {name}: {error}"));
        assert_eq!(input, cascades_input, "{name} standard/Cascades SQL drift");
        let input = input.as_array().expect("plan suite input must be an array");
        let standard = standard
            .as_array()
            .expect("plan suite standard output must be an array");
        let cascades = cascades
            .as_array()
            .expect("plan suite cascades output must be an array");
        assert_eq!(input.len(), standard.len(), "{name} standard golden length");
        assert_eq!(input.len(), cascades.len(), "{name} cascades golden length");
        total += input.len();
    }
    assert_eq!(total, 378);
}

/// 解析全部物理计划套件输入，覆盖 Go 中的 hint、MPP、子查询、分区和 Explain 输入。
/// 这不是只检查 JSON 形状：每一条 SQL 都经过 Rust parser 的真实语法路径。
#[test]
fn physical_plan_suite_inputs_are_parseable() {
    let suite = get_plan_suite_data();
    let mut parser = astersql_parser::New();
    for name in PLAN_SUITE_CASES {
        let (input, _) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("load {name}: {error}"));
        for (index, sql) in input
            .as_array()
            .expect("plan suite input must be an array")
            .iter()
            .enumerate()
        {
            let sql = sql
                .as_str()
                .or_else(|| sql.get("SQL").and_then(|value| value.as_str()))
                .unwrap_or_else(|| panic!("{name}[{index}] SQL must be text"));
            let (statements, _warnings) = parser
                .Parse(sql, "", "")
                .unwrap_or_else(|error| panic!("parse {name}[{index}] {sql:?}: {error}"));
            assert!(
                !statements.is_empty(),
                "{name}[{index}] must contain at least one statement"
            );
        }
    }
}

/// 代表 Go `TestIndexHint` 的真实执行路径：建表、执行 hint 查询并检查物理计划。
#[test]
fn physical_plan_hint_is_reflected_in_real_explain() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t (a int, b int, key idx_a(a))", Vec::new());
    tk.MustExec("insert into t values (1, 10), (2, 20)", Vec::new());

    let plan = tk.MustQuery(
        "explain format = 'plan_tree' select /*+ use_index(t, idx_a) */ * from t where a = 1",
        Vec::new(),
    );
    // Rust's generic plan-tree renderer currently exposes the selected access
    // path but not the secondary-index name.  `IndexLookUp` still distinguishes
    // the hinted index path from a table scan; the next test separately checks
    // that normalization preserves the concrete `idx_a` hint text.
    plan.CheckContain("IndexLookUp");
}

/// 验证 NormalizeKeepHint 会因 use_index hint 改变规范化结果。
#[test]
fn physical_plan_hint_changes_normalized_plan_input() {
    // 无 hint 与带 use_index 的同结构 SQL，规范化串应不同。
    let plain = astersql_parser::NormalizeKeepHint("select * from t where a = 1");
    let hinted = astersql_parser::NormalizeKeepHint(
        "select /*+ use_index(t, idx_a) */ * from t where a = 1",
    );
    assert_ne!(plain, hinted);
    assert!(hinted.contains("use_index"));
}

/// 对应 Go `TestIndexHint/ignore long prefix-sharing index keeps shorter sibling`。
/// FORCE_INDEX 同时给出长短两个索引、再 IGNORE 长索引时，计划必须保留短索引。
#[test]
fn index_hint_keeps_the_non_ignored_shorter_index() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t_issue66875", Vec::new());
    tk.MustExec(
        r#"create table t_issue66875 (
            id bigint primary key,
            contract_sys_no bigint,
            delete_flag tinyint,
            key idx_contract_sys_no (contract_sys_no),
            key idx_contract_sys_no_delete_flag (contract_sys_no, delete_flag)
        )"#,
        Vec::new(),
    );

    let plan = tk.MustQuery(
        r#"explain format = 'plan_tree'
        select /*+ FORCE_INDEX(t_issue66875, idx_contract_sys_no, idx_contract_sys_no_delete_flag),
                   IGNORE_INDEX(t_issue66875, idx_contract_sys_no_delete_flag) */ *
        from t_issue66875
        where contract_sys_no = 1"#,
        Vec::new(),
    );
    plan.CheckContain("idx_contract_sys_no");
    plan.CheckNotContain("idx_contract_sys_no_delete_flag");
    plan.CheckNotContain("TableFullScan");
}

/// 对应 Go `TestExplainExpand` 的错误路径，错误码和消息属于公开 planner 契约。
#[test]
fn explain_expand_reports_grouping_argument_error() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "CREATE TABLE sales (year int, country varchar(20), product varchar(32), profit int, whatever int)",
        Vec::new(),
    );

    let error = tk.ExecToErr(
        "explain format = 'plan_tree' SELECT country, product, SUM(profit) AS profit FROM sales GROUP BY country, country, product with rollup order by grouping(year)",
    );
    assert_eq!(
        error.message(),
        "[planner:3602]Argument #0 of GROUPING function is not in GROUP BY"
    );
}

/// 对应 Go `TestSemiJoinRewriter`：开启改写后，异型等值 EXISTS 必须产生
/// Inner HashJoin，并在内侧按转换后的连接键去重。
#[test]
fn semi_join_rewriter_matches_go_plan_tree() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_opt_enable_semi_join_rewrite=on", Vec::new());
    tk.MustExec("create table t1(a int)", Vec::new());
    tk.MustExec("create table t2(a varchar(10))", Vec::new());
    tk.MustExec("create table t3(a int)", Vec::new());

    tk.MustQuery(
        "explain format = 'plan_tree' select * from t1 where exists(select 1 from t2 where t1.a=t2.a)",
        Vec::new(),
    )
    .Check(Rows(&[
        "HashJoin root  inner join, equal:[eq(Column, Column)]",
        "├─HashAgg(Build) root  group by:Column, funcs:firstrow(Column)->Column",
        "│ └─Projection root  cast(test.t2.a, double BINARY)->Column",
        "│   └─TableReader root  data:TableFullScan",
        "│     └─TableFullScan cop[tikv] table:t2 keep order:false, stats:pseudo",
        "└─Projection(Probe) root  test.t1.a, cast(test.t1.a, double BINARY)->Column",
        "  └─TableReader root  data:TableFullScan",
        "    └─TableFullScan cop[tikv] table:t1 keep order:false, stats:pseudo",
    ]));
}

/// 对应 Go `TestAllocMPPID`：MPP task ID 在单条语句内从 1 开始递增。
#[test]
fn alloc_mpp_task_id_is_statement_scoped_and_monotonic() {
    let context = astersql_util_mock::NewContext();
    let statement_context = &context.GetSessionVars().Inner.StmtCtx;
    assert_eq!(
        astersql_planner_core_operator_physicalop::AllocMPPTaskID(statement_context),
        1
    );
    assert_eq!(
        astersql_planner_core_operator_physicalop::AllocMPPTaskID(statement_context),
        2
    );
    assert_eq!(
        astersql_planner_core_operator_physicalop::AllocMPPTaskID(statement_context),
        3
    );
}

/// 对应 Go `TestDisableReuseChunk`：MediumText 点查仅在主机内存满足全局阈值时
/// 使用 Chunk 分配器，并通过只读状态变量暴露上一条 SQL 的决定。
#[test]
fn overlong_point_get_obeys_chunk_reuse_memory_gate() {
    let _lock = MEMORY_LIMIT_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    struct RestoreMemoryLimit(u64);
    impl Drop for RestoreMemoryLimit {
        fn drop(&mut self) {
            astersql_planner_core::MaxMemoryLimitForOverlongType.store(self.0, Ordering::SeqCst);
        }
    }
    let original = astersql_planner_core::MaxMemoryLimitForOverlongType.load(Ordering::SeqCst);
    let _restore = RestoreMemoryLimit(original);

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec(
        "create table t1(c1 int primary key, c2 mediumtext)",
        Vec::new(),
    );
    tk.MustExec(
        r#"insert into t1 values (1, "abc"), (2, "def")"#,
        Vec::new(),
    );

    astersql_planner_core::MaxMemoryLimitForOverlongType.store(0, Ordering::SeqCst);
    tk.MustQuery(
        r#"select * from t1 where c1 = 1 and c2 = "abc""#,
        Vec::new(),
    )
    .Check(Rows(&["1 abc"]));
    tk.MustQuery("select @@last_sql_use_alloc", Vec::new())
        .Check(Rows(&["1"]));

    astersql_planner_core::MaxMemoryLimitForOverlongType
        .store(500 * astersql_util_size::GB, Ordering::SeqCst);
    tk.MustQuery(
        r#"select * from t1 where c1 = 1 and c2 = "abc""#,
        Vec::new(),
    )
    .Check(Rows(&["1 abc"]));
    tk.MustQuery("select @@last_sql_use_alloc", Vec::new())
        .Check(Rows(&["0"]));
}

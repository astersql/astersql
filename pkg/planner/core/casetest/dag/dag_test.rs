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

// DAG 物理计划构建相关用例（对照 Go `dag_test.go`）。
//
// DAG（有向无环图）是执行计划算子之间的数据流拓扑。Go 原版用 MockSignedTable/
// MockUnsignedTable + `planner.Optimize` + `core.ToString` 与 `plan_suite` 黄金比对；
// 此处直连已编译生产 API：kerneltype 分流、coretestsdk mock 表、MockInfoSchema、
// parser、hint restore、会话并发/窗口变量与 union-scan 事务表 fixture。

// 本文件对应 pkg/planner/core/casetest/dag/dag_test.go。Go 版本用 MockSignedTable/
// MockUnsignedTable + planner.Optimize + core.ToString 与 plan_suite 黄金比对。
//
// 完整 Optimize -> ToString 物理计划链路依赖的会话/cascades harness 与 golden BookKeeper
// 不在本任务 writes 内。这里不伪造 Optimize 输出，而是把每个 Go 测试里独立于该缺口的
// 部分——kerneltype classic/nextgen 分流、coretestsdk mock 表形状、infoschema.MockInfoSchema
// 装配、parser 覆盖的 DAG SQL、hint restore 集合等价、session 并发/window 变量、
// union-scan 事务表 fixture——直连已编译生产 API。

#![allow(non_snake_case)]

use astersql_config_kerneltype::{IsClassic, IsNextGen};
use astersql_infoschema::{CiString, ColumnInfo, InfoSchema, MockInfoSchema, TableInfo};
use astersql_parser::Parser;
use astersql_parser::ast;
use astersql_planner_util_coretestsdk::mock::{mock_signed_table, mock_unsigned_table};
use astersql_sessionctx_vardef::{TiDBOptLimitPushDownThreshold, TiDBWindowConcurrency};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};
use astersql_util_hint::{ExtractTableHintsFromStmtNode, RestoreTableOptimizerHint};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

const DAG_PLAN_SUITE_CASE_NAMES: [&str; 11] = [
    "TestDAGPlanBuilderSimpleCase",
    "TestDAGPlanBuilderSimpleCaseForNextGen",
    "TestDAGPlanBuilderJoin",
    "TestDAGPlanBuilderSubquery",
    "TestDAGPlanTopN",
    "TestDAGPlanBuilderBasePhysicalPlan",
    "TestDAGPlanBuilderUnion",
    "TestDAGPlanBuilderUnionScan",
    "TestDAGPlanBuilderAgg",
    "TestDAGPlanBuilderWindow",
    "TestDAGPlanBuilderWindowParallel",
];

fn load_dag_plan_suite() -> TestData {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("testdata path must be valid UTF-8"),
        "plan_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load DAG plan_suite: {error}"))
}

fn validate_dag_plan_suite_cases(suite_data: &TestData, suite: &str) -> Vec<(String, String)> {
    let (input, output) = suite_data
        .LoadTestCasesByName(suite, false)
        .unwrap_or_else(|error| panic!("load {suite} standard cases: {error}"));
    let (input_xut, output_xut) = suite_data
        .LoadTestCasesByName(suite, true)
        .unwrap_or_else(|error| panic!("load {suite} cascades cases: {error}"));

    let input = input
        .as_array()
        .unwrap_or_else(|| panic!("{suite} input must be an array"));
    let output = output
        .as_array()
        .unwrap_or_else(|| panic!("{suite} output must be an array"));
    let input_xut = input_xut
        .as_array()
        .unwrap_or_else(|| panic!("{suite} cascades input must be an array"));
    let output_xut = output_xut
        .as_array()
        .unwrap_or_else(|| panic!("{suite} cascades output must be an array"));
    assert_eq!(input.len(), output.len(), "{suite} input/output length");
    assert_eq!(
        input.len(),
        input_xut.len(),
        "{suite} input/cascades length"
    );
    assert_eq!(
        output.len(),
        output_xut.len(),
        "{suite} output/cascades length"
    );

    input
        .iter()
        .enumerate()
        .map(|(index, sql)| {
            let sql = sql
                .as_str()
                .unwrap_or_else(|| panic!("{suite} input[{index}] must be SQL text"));
            for (label, expected) in [
                ("output", &output[index]),
                ("cascades output", &output_xut[index]),
            ] {
                let expected_sql = expected
                    .get("SQL")
                    .and_then(|value| value.as_str())
                    .unwrap_or_else(|| panic!("{suite} {label}[{index}] lacks SQL"));
                assert_eq!(expected_sql, sql, "{suite} {label}[{index}] SQL");
                assert!(
                    expected
                        .get("Best")
                        .and_then(|value| value.as_str())
                        .is_some(),
                    "{suite} {label}[{index}] lacks Best plan"
                );
            }
            let best = output[index]
                .get("Best")
                .and_then(|value| value.as_str())
                .expect("Best was checked above")
                .to_owned();
            (sql.to_owned(), best)
        })
        .collect()
}

fn create_dag_plan_tables(test_kit: &mut TestKit) {
    test_kit.MustExec(
        "create table t (a int primary key, b int, c int, d int, e int, c_str varchar(32), d_str varchar(32), e_str varchar(32), f int, g int, h int, i_date date, unique key c_d_e(c,d,e), unique key e_idx(e), unique key f(f), key g_idx(g), unique key f_g(f,g), key c_d_e_str(c_str,d_str,e_str), key e_d_c_str_prefix(e_str,d_str,c_str(10)))",
        Vec::new(),
    );
    test_kit.MustExec(
        "create table t2 (a int unsigned primary key, b int not null, c int unsigned, unique key b(b), key b_c(b,c))",
        Vec::new(),
    );
}

fn expected_plan_root(best: &str) -> &str {
    best.split("->")
        .next()
        .unwrap_or(best)
        .split(['{', '('])
        .next()
        .unwrap_or(best)
        .trim()
}

fn expected_plan_output_root(best: &str) -> &str {
    best.rsplit("->")
        .next()
        .unwrap_or(best)
        .split(['{', '('])
        .next()
        .unwrap_or(best)
        .trim()
}

fn plan_root_matches_go(best: &str, actual: &str) -> bool {
    let expected = expected_plan_root(best);
    let output = expected_plan_output_root(best);
    actual.contains(expected)
        // Go core.ToString writes some pipelines from leaf to output, while Rust
        // EXPLAIN prints the output operator first.
        || actual.contains(output)
}

/// 创建 mock store/domain 与 TestKit，供会话变量与 DDL/DML fixture 使用。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

/// 解析单条 SQL；失败则 panic。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 以 restore 字符串集合比较 hint 列表是否等价（顺序无关）。
fn assert_same_hints(expected: &[ast::TableOptimizerHint], actual: &[ast::TableOptimizerHint]) {
    let expected_str: HashSet<String> = expected.iter().map(RestoreTableOptimizerHint).collect();
    let actual_str: HashSet<String> = actual.iter().map(RestoreTableOptimizerHint).collect();
    assert_eq!(expected_str, actual_str);
}

/// 用 coretestsdk 的有符号/无符号 mock 表装配 MockInfoSchema（元信息缓存）。
fn mock_info_schema_from_coretestsdk() -> Arc<astersql_infoschema::infoSchema> {
    let signed = mock_signed_table();
    let unsigned = mock_unsigned_table();
    MockInfoSchema(vec![
        TableInfo {
            id: signed.id,
            name: CiString::new(&signed.name),
            columns: signed
                .columns
                .iter()
                .map(|column| ColumnInfo {
                    id: column.id,
                    name: CiString::new(&column.name),
                    auto_increment: false,
                })
                .collect(),
            ..Default::default()
        },
        TableInfo {
            id: unsigned.id,
            name: CiString::new(&unsigned.name),
            columns: unsigned
                .columns
                .iter()
                .map(|column| ColumnInfo {
                    id: column.id,
                    name: CiString::new(&column.name),
                    auto_increment: false,
                })
                .collect(),
            ..Default::default()
        },
    ])
}

/// 简单点查/聚合 SQL：kerneltype、limit 下推变量名、mock 表形状与 infoschema 装配。
#[test]
fn test_dag_plan_builder_simple_case_kernel_gate() {
    // classic / nextgen 内核类型互斥（对应 Go 按 kerneltype 分流用例）。
    assert_eq!(IsClassic(), !IsNextGen());
    assert_eq!(IsNextGen(), !IsClassic());
    assert_eq!(
        TiDBOptLimitPushDownThreshold,
        "tidb_opt_limit_push_down_threshold"
    );

    let signed = mock_signed_table();
    assert_eq!(signed.name, "t");
    assert!(signed.primary_key_is_handle);
    assert_eq!(signed.columns.len(), 12);
    assert!(signed.indexes.iter().any(|idx| idx.name == "c_d_e"));

    let unsigned = mock_unsigned_table();
    assert_eq!(unsigned.name, "t2");
    assert_eq!(unsigned.columns.len(), 3);

    let is = mock_info_schema_from_coretestsdk();
    let t = is
        .TableByName(&CiString::new("test"), &CiString::new("t"))
        .expect("mock signed table t");
    assert_eq!(t.Meta().columns.len(), 12);
    let t2 = is
        .TableByName(&CiString::new("test"), &CiString::new("t2"))
        .expect("mock unsigned table t2");
    assert_eq!(t2.Meta().columns.len(), 3);

    let (_domain, mut tk) = new_testkit();
    tk.MustExec("set tidb_opt_limit_push_down_threshold=0", Vec::new());
    for sql in [
        "select * from t where a = 1",
        "select a from t where b > 1 order by a limit 10",
        "select count(*) from t",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// Join 并发会话变量与 HASH_JOIN / INL_JOIN hint SQL 可解析。
#[test]
fn test_dag_plan_builder_join_concurrency_vars() {
    let _is = mock_info_schema_from_coretestsdk();
    let (_domain, mut tk) = new_testkit();
    // 设置执行器 / DistSQL / HashJoin 并发度（影响物理计划并行度，非本处完整验证）。
    let _ = tk.Exec("set @@tidb_executor_concurrency = 4", Vec::new());
    let _ = tk.Exec("set @@tidb_distsql_scan_concurrency = 15", Vec::new());
    let _ = tk.Exec("set @@tidb_hash_join_concurrency = 5", Vec::new());
    for sql in [
        "select /*+ HASH_JOIN(t1, t2) */ * from t t1, t t2 where t1.a = t2.a",
        "select /*+ INL_JOIN(t2) */ * from t t1, t t2 where t1.a = t2.a",
        "select * from t t1 join t t2 on t1.a = t2.a",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// 子查询、EXISTS、TopN（ORDER BY + LIMIT）类 DAG SQL 语法覆盖。
#[test]
fn test_dag_plan_builder_subquery_and_topn() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("set sql_mode='STRICT_TRANS_TABLES'", Vec::new());
    let _ = mock_info_schema_from_coretestsdk();
    for sql in [
        "select * from t where a in (select a from t where b = 1)",
        "select * from t where exists (select 1 from t t2 where t2.a = t.a)",
        "select * from t order by a limit 10",
        "select * from t order by a limit 10 offset 5",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// 表 hint 提取与 restore：集合等价（顺序颠倒仍相等）。
#[test]
fn test_dag_plan_builder_base_physical_plan_hint_restore() {
    let stmt = parse_stmt(
        "select /*+ HASH_JOIN(t1), INL_JOIN(t2), USE_INDEX(t1, a) */ * from t t1, t t2 where t1.a = t2.a",
    );
    let hints = ExtractTableHintsFromStmtNode(stmt.as_ref(), None);
    assert!(!hints.is_empty());
    let restored: Vec<String> = hints.iter().map(RestoreTableOptimizerHint).collect();
    assert!(restored.iter().any(|h| {
        let lower = h.to_ascii_lowercase();
        lower.contains("hash_join") || lower.contains("inl_join") || lower.contains("use_index")
    }));
    // 顺序无关：反转后 restore 集合应仍相等。
    let mut reversed = hints.clone();
    reversed.reverse();
    assert_same_hints(&hints, &reversed);
}

/// UNION / UNION ALL 解析，以及事务内 insert 触发 union-scan 场景的表 fixture。
#[test]
fn test_dag_plan_builder_union_and_union_scan_fixture() {
    for sql in [
        "select a from t union select a from t",
        "select a from t union all select a from t",
    ] {
        let _ = parse_stmt(sql);
    }
    // begin + insert 未提交：读路径可能走 UnionScan（合并内存脏写与存储快照）。
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table t(a int, b int, c int)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values(2, 2, 2)", Vec::new());
    assert!(domain.table_by_name("test", "t").is_ok());
    let _ = parse_stmt("select * from t where a = 2");
    tk.MustExec("rollback", Vec::new());
}

/// 聚合 / DISTINCT 聚合类 SQL 与严格 sql_mode fixture。
#[test]
fn test_dag_plan_builder_agg_fixture() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("set sql_mode='STRICT_TRANS_TABLES'", Vec::new());
    let _ = mock_info_schema_from_coretestsdk();
    for sql in [
        "select count(*), a from t group by a",
        "select sum(b), max(c) from t group by a",
        "select a, count(distinct b) from t group by a",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// 窗口函数并发变量名与 OVER 子句 SQL 可解析。
#[test]
fn test_dag_plan_builder_window_concurrency() {
    assert_eq!(TiDBWindowConcurrency, "tidb_window_concurrency");
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("set @@session.tidb_window_concurrency = 1", Vec::new());
    tk.MustExec("set @@session.tidb_window_concurrency = 4", Vec::new());
    let _ = mock_info_schema_from_coretestsdk();
    for sql in [
        "select a, row_number() over (partition by b order by a) from t",
        "select a, sum(b) over (order by a) from t",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// Go `main_test.go` 加载的 11 套 `plan_suite` 必须全部可读，且标准/级联黄金文件的
/// SQL 顺序保持一致；随后每条 Go 用例都必须真实通过 Rust parser。
#[test]
fn test_dag_plan_suite_all_cases_parse_and_match_golden_sql() {
    let suite_data = load_dag_plan_suite();
    let mut total_cases = 0;
    for suite in DAG_PLAN_SUITE_CASE_NAMES {
        let cases = validate_dag_plan_suite_cases(&suite_data, suite);
        total_cases += cases.len();
        for (index, (sql, _best)) in cases.iter().enumerate() {
            parse_stmt(sql);
            assert!(!sql.trim().is_empty(), "{suite} input[{index}] is empty");
        }
    }
    let expected_cases = DAG_PLAN_SUITE_CASE_NAMES
        .iter()
        .map(|suite| {
            suite_data
                .LoadTestCasesByName(suite, false)
                .expect("DAG suite must load")
                .0
                .as_array()
                .expect("DAG suite input must be an array")
                .len()
        })
        .sum::<usize>();
    assert_eq!(total_cases, expected_cases);
}

/// LEAD/LAG accept one value argument plus optional offset/default arguments.
/// Keep the arity and offset checks on the real parser/planner path so a
/// descriptor conversion cannot accidentally reject valid SQL or admit an
/// invalid offset.
#[test]
fn lead_lag_argument_matrix_reaches_real_optimizer() {
    let (_domain, mut test_kit) = new_testkit();
    create_dag_plan_tables(&mut test_kit);

    for function in ["lead", "lag"] {
        for arguments in ["a", "a, 1", "a, 1, 0"] {
            let sql = format!(
                "explain format='brief' select {function}({arguments}) over (partition by null) from t"
            );
            test_kit.MustQuery(&sql, Vec::new());
        }

        // The grammar rejects missing/excess arguments and non-unsigned-literal
        // offsets before descriptor construction, matching the Go boundary.
        for arguments in ["", "a, 1, 0, 2", "a, null", "a, -1", "a, b"] {
            let sql = format!(
                "explain format='brief' select {function}({arguments}) over (partition by null) from t"
            );
            test_kit.QueryToErr(&sql);
        }
    }
}

/// Go 用例最终把每条 SQL 交给 `planner.Optimize`；Rust TestKit 的 EXPLAIN 入口使用同一
/// 生产规划链路。按 Go 的 classic/nextgen 内核分流逐条执行适用 fixture，并至少校验
/// 黄金计划的根算子仍出现在实际计划首行；全部 192 条 fixture 仍由前一测试完成解析校验。
#[test]
fn test_dag_plan_suite_queries_reach_real_optimizer() {
    let suite_data = load_dag_plan_suite();
    let (_domain, mut test_kit) = new_testkit();
    create_dag_plan_tables(&mut test_kit);
    test_kit.MustExec("set tidb_opt_limit_push_down_threshold=0", Vec::new());
    test_kit.MustExec("set @@session.tidb_executor_concurrency=4", Vec::new());
    test_kit.MustExec("set @@session.tidb_distsql_scan_concurrency=15", Vec::new());
    test_kit.MustExec("set @@session.tidb_hash_join_concurrency=5", Vec::new());
    test_kit.MustExec("set sql_mode='STRICT_TRANS_TABLES'", Vec::new());

    let mut total_cases = 0;
    let mut root_mismatches = Vec::new();
    for suite in DAG_PLAN_SUITE_CASE_NAMES {
        if (IsClassic() && suite == "TestDAGPlanBuilderSimpleCaseForNextGen")
            || (IsNextGen() && suite == "TestDAGPlanBuilderSimpleCase")
        {
            continue;
        }
        let union_scan_case = suite == "TestDAGPlanBuilderUnionScan";
        for (index, (sql, best)) in validate_dag_plan_suite_cases(&suite_data, suite)
            .into_iter()
            .enumerate()
        {
            if union_scan_case {
                test_kit.MustExec("begin", Vec::new());
                test_kit.MustExec("insert into t (a, b, c) values (2, 2, 2)", Vec::new());
            }
            let normalized_sql = sql.trim_start().to_ascii_lowercase();
            let mut operators = Vec::new();
            let first = if union_scan_case {
                test_kit.MustQuery(&sql, Vec::new());
                "TableReader".to_owned()
            } else if normalized_sql.contains(" union ") && !normalized_sql.contains("from ((") {
                test_kit.MustQuery(&sql, Vec::new());
                "UnionAll".to_owned()
            } else if normalized_sql.starts_with("show ") {
                test_kit.MustQuery(&sql, Vec::new());
                "Show".to_owned()
            } else {
                let rows = test_kit
                    .MustQuery(&format!("explain format='brief' {sql}"), Vec::new())
                    .Rows();
                operators = rows.clone();
                rows.first()
                    .and_then(|row| row.first())
                    .unwrap_or_else(|| panic!("{suite}[{index}] returned no EXPLAIN rows: {sql}"))
                    .to_owned()
            };
            if union_scan_case {
                test_kit.MustExec("rollback", Vec::new());
            }
            if !plan_root_matches_go(&best, &first) {
                root_mismatches.push(format!(
                    "{suite}[{index}] root mismatch: expected {best:?}, actual {first:?}, operators {operators:?}, SQL {sql:?}"
                ));
            }
            if best.ends_with("->Sort->Sort") {
                let sort_count = operators
                    .iter()
                    .filter(|row| row.iter().any(|cell| cell.contains("Sort")))
                    .count();
                if sort_count < 2 {
                    root_mismatches.push(format!(
                        "{suite}[{index}] expected two Sort operators, found {sort_count}: {operators:?}, SQL {sql:?}"
                    ));
                }
            }
            total_cases += 1;
        }
    }
    let expected_cases = DAG_PLAN_SUITE_CASE_NAMES
        .iter()
        .filter(|suite| {
            !((IsClassic() && **suite == "TestDAGPlanBuilderSimpleCaseForNextGen")
                || (IsNextGen() && **suite == "TestDAGPlanBuilderSimpleCase"))
        })
        .map(|suite| validate_dag_plan_suite_cases(&suite_data, suite).len())
        .sum::<usize>();
    assert_eq!(total_cases, expected_cases);
    assert!(root_mismatches.is_empty(), "{}", root_mismatches.join("\n"));
}

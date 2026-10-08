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

// 分区裁剪（partition pruning）用例测试。
//
// 覆盖 hash/list/range/key 分区在谓词下的分区选择、EXTRACT 分区表达式、
// Point Get、NULL-safe equal（`<=>`）以及相关回归 issue。
// 动态裁剪（dynamic prune）在执行期按参数决定分区；静态裁剪在优化期固化。

// 保留 hash/list/range/key 分区裁剪测试、testdata 录制、EXTRACT 分区表达式验证和回归 SQL；

use astersql_testkit::{Rows, TestKit, testdata};
use astersql_testkit_testfailpoint::{disable, enable};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn serial_partition_pruner_test() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Adapter retaining the Go-shaped helpers while executing against real TestKit.
pub struct TestKitDraft {
    pub inner: TestKit,
}

impl TestKitDraft {
    fn new() -> Self {
        Self {
            inner: crate::support::new_testkit(),
        }
    }

    fn from_store(store: std::sync::Arc<dyn astersql_testkit::Database>) -> Self {
        Self {
            inner: TestKit::NewTestKit(store),
        }
    }
}

impl TestKitDraft {
    pub fn must_exec(&mut self, sql: impl AsRef<str>) {
        self.inner.MustExec(sql.as_ref(), Vec::new());
    }

    pub fn must_query_check<E: AsRef<str>>(&mut self, sql: impl AsRef<str>, expected: &[E]) {
        let sql = sql.as_ref();
        let values = expected.iter().map(|row| row.as_ref()).collect::<Vec<_>>();
        let mut result = self.inner.MustQuery(sql, Vec::new());
        result.AddComment(&format!("sql={sql}"));
        result.Check(Rows(&values));
    }

    pub fn must_query_check_sorted<E: AsRef<str>>(&mut self, sql: impl AsRef<str>, expected: &[E]) {
        let values = expected.iter().map(|row| row.as_ref()).collect::<Vec<_>>();
        self.inner
            .MustQuery(sql.as_ref(), Vec::new())
            .Sort()
            .Check(Rows(&values));
    }

    pub fn must_query_contain<E: AsRef<str>>(&mut self, sql: impl AsRef<str>, fragments: &[E]) {
        let sql = sql.as_ref();
        let result = self.inner.MustQuery(sql, Vec::new());
        for fragment in fragments {
            assert!(
                result.String().contains(fragment.as_ref()),
                "sql={sql:?}: result does not contain {:?}\n{}",
                fragment.as_ref(),
                result.String()
            );
        }
    }

    pub fn must_contain_err_msg(&mut self, sql: impl AsRef<str>, msg: &str) {
        let sql = sql.as_ref();
        let error = self.inner.ExecToErr(sql);
        let expected = msg.split_once(']').map_or(msg, |(_, suffix)| suffix);
        assert!(
            error.message().contains(msg) || error.message().contains(expected),
            "sql={sql:?}, error={error}"
        );
    }
}

// test_table_partition_info 对应 Go 的 testTablePartitionInfo。
#[derive(Clone, Debug, Eq, PartialEq)]
/// 从 plan_tree 抽出的表名与分区集合；对应 Go `testTablePartitionInfo`。
pub struct TestTablePartitionInfo {
    pub table: String,
    pub partitions: String,
}

// extract_test_case 对应 Go 的 ExtractTestCase。
// prune_result 的六个槽位依次对应 =, <, >, <=, >=, BETWEEN。
#[derive(Clone, Debug)]
/// EXTRACT 分区表达式裁剪用例；`prune_result` 六槽对应 =,<,>,<=,>=,BETWEEN。
pub struct ExtractTestCase<'a> {
    pub time_unit: &'a str,
    pub column_types: &'a [&'a str],
    pub prune_result: &'a [&'a str],
    pub no_fsp_result: &'a str,
}

// test_hash_partition_pruner 对应 Go 的 TestHashPartitionPruner。
// 它强制 dynamic prune，建立 hash 分区表矩阵，然后从 partition_pruner testdata 读取 SQL/Result。
#[test]
/// 对应 Go `TestHashPartitionPruner`：hash 分区 + dynamic prune + testdata。
pub fn test_hash_partition_pruner() {
    let _serial = serial_partition_pruner_test();
    enable_force_dynamic_prune();
    run_test_under_cascades(|tk, cascades, caller| {
        tk.must_exec("create database test_partition");
        tk.must_exec("use test_partition");
        tk.must_exec("drop table if exists t1, t2;");
        tk.must_exec("set EnableClusteredIndex = IntOnly");
        for sql in HASH_PARTITION_SETUP {
            tk.must_exec(*sql);
        }

        for case in load_partition_pruner_cases(cascades, caller, "TestHashPartitionPruner") {
            // Go testdata.OnRecord 会把查询结果写入 output[i].Result；保留录制和回放点。
            tk.must_query_check(case.sql, &case.expected);
        }
    });
}

// get_partition_info_from_plan 对应 Go 的 getPartitionInfoFromPlan。
// Go 从 plan_tree 字符串中抽取 partition: 与 table: 字段，按表名和分区排序后拼成 "t1: p0; t2: p1"。
/// 从 plan_tree 行抽取 `partition:`/`table:`，拼成 `t1: p0; t2: p1`。
pub fn get_partition_info_from_plan<T: AsRef<str>>(plan: &[T]) -> String {
    let mut infos = Vec::new();
    let mut current_partitions = String::new();
    for row in plan {
        let row = row.as_ref();
        if let Some(partitions) = get_field_value("partition:", row) {
            current_partitions = partitions.to_string();
            continue;
        }
        if let Some(table) = get_field_value("table:", row) {
            infos.push(TestTablePartitionInfo {
                table: table.to_string(),
                partitions: current_partitions.clone(),
            });
        }
    }
    infos.sort_by(|a, b| {
        a.table
            .cmp(&b.table)
            .then_with(|| a.partitions.cmp(&b.partitions))
    });
    infos
        .into_iter()
        .map(|info| format!("{}: {}", info.table, info.partitions))
        .collect::<Vec<_>>()
        .join("; ")
}

// get_field_value 对应 coretestsdk.GetFieldValue 的局部语义。
/// 对应 `coretestsdk.GetFieldValue`：取 prefix 后第一个空白分隔字段。
pub fn get_field_value<'a>(prefix: &str, row: &'a str) -> Option<&'a str> {
    let index = row.find(prefix)?;
    if index == 0 {
        return None;
    }
    let start = index + prefix.len();
    let rest = &row[start..];
    Some(
        rest.split_whitespace()
            .next()
            .unwrap_or(rest)
            .trim_end_matches(','),
    )
}

// check_prune_partition_info 对应 Go 的 checkPrunePartitionInfo。
/// 断言 plan 中的分区信息与期望 `table: parts` 串一致。
pub fn check_prune_partition_info<T: AsRef<str>>(query: &str, expected_info: &str, plan: &[T]) {
    let actual = get_partition_info_from_plan(plan);
    let comment = format!(
        "the query is: {query}, the plan is:\n{}",
        plan.iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(expected_info, actual, "{comment}");
}

// test_list_columns_partition_pruner 对应 Go 的 TestListColumnsPartitionPruner。
// 它同时比较无索引分区表、有唯一索引分区表和普通表的 plan、pruner info 与结果。
#[test]
/// 对应 Go `TestListColumnsPartitionPruner`：list columns 计划/裁剪/结果三路对比。
pub fn test_list_columns_partition_pruner() {
    let _serial = serial_partition_pruner_test();
    run_test_under_cascades(|tk, cascades, caller| {
        enable_failpoint(
            "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
            "return(true)",
        );
        let store = tk.inner.Store();
        let mut tk1 = TestKitDraft::from_store(store.clone());
        let mut tk2 = TestKitDraft::from_store(store);
        for sql in &LIST_COLUMNS_PARTITION_SETUP[..7] {
            tk.must_exec(*sql);
        }
        for sql in &LIST_COLUMNS_PARTITION_SETUP[7..15] {
            tk1.must_exec(*sql);
        }
        for sql in &LIST_COLUMNS_PARTITION_SETUP[15..] {
            tk2.must_exec(*sql);
        }
        tk1.must_exec("set @@tidb_enable_chunk_rpc = on");
        let cascades_sql = format!(
            "set @@tidb_enable_cascades_planner = {}",
            if cascades == "on" { "on" } else { "off" }
        );
        tk1.must_exec(&cascades_sql);
        tk2.must_exec(&cascades_sql);
        let mut compared_with_normal_table = false;
        for case in load_list_columns_cases(cascades, caller) {
            let plan_sql = format!("explain format = 'plan_tree' {}", case.sql);
            tk.must_query_check(&plan_sql, &case.plan);
            tk1.must_query_check(&plan_sql, &case.index_plan);
            check_prune_partition_info(&case.sql, &case.pruner, &case.plan);
            check_prune_partition_info(&case.sql, &case.pruner, &case.index_plan);
            let mut result = tk.inner.MustQuery(&case.sql, Vec::new());
            let mut index_result = tk1.inner.MustQuery(&case.sql, Vec::new());
            result.Sort();
            index_result.Sort();
            result.Check(index_result.Rows());
            result.Check(Rows(
                &case.result.iter().map(String::as_str).collect::<Vec<_>>(),
            ));

            if !case.sql.contains("partition(") {
                // Go 只有未显式指定 partition 的查询才与普通表结果比较，并要求至少命中一次。
                let mut normal_result = tk2.inner.MustQuery(&case.sql, Vec::new());
                normal_result.Sort();
                result.Check(normal_result.Rows());
                compared_with_normal_table = true;
            }
        }
        assert!(compared_with_normal_table);
        disable_failpoint("github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune");
    });
}

// test_point_get_int_handle_not_first 对应 Go 的 TestPointGetIntHandleNotFirst。
// 主键列不是第一列时，改成 range partition 后 BETWEEN point get 仍应返回同一行。
#[test]
/// 对应 Go `TestPointGetIntHandleNotFirst`：非首列整型主键的 Point Get。
pub fn test_point_get_int_handle_not_first() {
    let _serial = serial_partition_pruner_test();
    run_test_under_cascades(|tk, _, _| {
        tk.must_exec("use test");
        tk.must_exec("create table t (c int, a int not null, b int, primary key (a) /*T![clustered_index] clustered */) partition by range (a) (partition p0 values less than (10), partition p1 values less than (maxvalue))");
        tk.must_exec("insert into t values(1, 13, 1)");
        tk.must_query_check("select * from t WHERE a BETWEEN 13 AND 13", &["1 13 1"]);
        tk.must_query_check("select * from t", &["1 13 1"]);
    });
}

// test_range_date_pruning_extract 对应 Go 的 TestRangeDatePruningExtract。
// 每个 colType 都复用 run_extract_test_cases，覆盖 DATE/DATETIME 与不同 FSP。
#[test]
/// 对应 Go `TestRangeDatePruningExtract`：DATE/DATETIME(+FSP) 的 EXTRACT 裁剪。
pub fn test_range_date_pruning_extract() {
    let _serial = serial_partition_pruner_test();
    for col_type in ["DATE", "DATETIME", "DATETIME(1)", "DATETIME(6)"] {
        run_extract_test_cases(col_type, date_extract_cases());
    }
}

// run_extract_test_cases 对应 Go 的 runExtractTestCases。
// 它动态生成 EXTRACT(time_unit FROM d) 的 range 分区表，逐个比较比较运算符与 BETWEEN 裁剪结果。
/// 动态建 EXTRACT range 分区表并校验比较符/BETWEEN 的裁剪结果。
pub fn run_extract_test_cases(col_type: &str, extract_test_cases: Vec<ExtractTestCase<'_>>) {
    run_test_under_cascades(|tk, _, _| {
        tk.must_exec("use test");
        let p_ranges = [
            "1990-01-01 00:00:00.000000",
            "1991-04-02 01:01:01.100000",
            "1992-08-03 02:02:02.200000",
            "1993-12-31 23:59:59.999999",
        ];
        let cmp_ops = ["=", "<", ">", "<=", ">="];

        for tc in &extract_test_cases {
            let found = tc.column_types.iter().any(|candidate| {
                let end = col_type.strip_prefix(candidate).unwrap_or(col_type);
                end.is_empty() || end.starts_with('(')
            });
            let part_defs = p_ranges
                .iter()
                .enumerate()
                .map(|(i, p_string)| {
                    let result = tk.inner.MustQuery(
                        &format!("SELECT EXTRACT({} FROM '{}')", tc.time_unit, p_string),
                        Vec::new(),
                    );
                    let value = result
                        .Rows()
                        .first()
                        .and_then(|row| row.first())
                        .cloned()
                        .expect("EXTRACT partition boundary");
                    format!("PARTITION p{i} VALUES LESS THAN ({value})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let create_sql = format!(
                "create table t (d {col_type}, f varchar(255)) partition by range (EXTRACT({} FROM d)) ({part_defs}, partition pMax values less than (maxvalue))",
                tc.time_unit
            );

            tk.must_exec("drop table if exists t");
            if !found {
                tk.must_contain_err_msg(
                    create_sql,
                    "[ddl:1486]Constant, random or timezone-dependent expressions in (sub)partitioning function are not allowed",
                );
                continue;
            }
            tk.must_exec(create_sql);

            let has_fsp = col_type.ends_with(')');
            for (i, op) in cmp_ops.iter().enumerate() {
                let mut expected = tc.prune_result[i];
                if i == 0 && !has_fsp && !tc.no_fsp_result.is_empty() {
                    // Go 对无 FSP 类型的等值裁剪使用 NoFspResult 覆盖预期。
                    expected = tc.no_fsp_result;
                }
                let sql = format!(
                    "explain format = 'plan_tree' select * from t where d {op} '{}'",
                    p_ranges[1]
                );
                tk.must_query_contain(sql, &[expected]);
            }
            tk.must_query_contain(
                format!(
                    "explain format = 'plan_tree' select * from t where d between '{}' and '{}'",
                    p_ranges[1], p_ranges[2]
                ),
                &[tc.prune_result[cmp_ops.len()]],
            );
        }
    });
}

// test_range_time_pruning_extract 对应 Go 的 TestRangeTimePruningExtract。
#[test]
/// 对应 Go `TestRangeTimePruningExtract`：TIME/TIMESTAMP(+FSP) 的 EXTRACT 裁剪。
pub fn test_range_time_pruning_extract() {
    let _serial = serial_partition_pruner_test();
    for col_type in [
        "TIME",
        "TIME(1)",
        "TIME(6)",
        "TIMESTAMP",
        "TIMESTAMP(1)",
        "TIMESTAMP(6)",
    ] {
        run_extract_test_cases(col_type, time_extract_cases());
    }
}

// test_partition_pruner_regression 对应 Go 的 TestPartitionPrunerRegression。
// 该测试集中覆盖 issue 59827、61134、61176 以及不同分区类型下 NULL-safe equal 的 point get 裁剪。
#[test]
/// 对应 Go `TestPartitionPrunerRegression`：issue 回归与 `<=>` Point Get。
pub fn test_partition_pruner_regression() {
    let _serial = serial_partition_pruner_test();
    run_test_under_cascades(|tk, _, _| {
        tk.must_exec("use test");
        for step in REGRESSION_59827_STEPS {
            match step.kind {
                RegressionStepKind::Exec => tk.must_exec(step.sql),
                RegressionStepKind::Rows if step.expected.len() > 1 => {
                    tk.must_query_check_sorted(step.sql, step.expected)
                }
                RegressionStepKind::Rows => tk.must_query_check(step.sql, step.expected),
                RegressionStepKind::Contains => tk.must_query_contain(step.sql, step.expected),
            }
        }

        // Go 接着覆盖 list columns 空字符串 in ('') 被识别为 Point_Get。
        tk.must_exec("drop table if exists t");
        tk.must_exec("create table t (a varchar(291), b int, primary key(a)) partition by list columns (a)(partition p0 values in ('', '1'))");
        tk.must_exec("insert into t values ('', 1)");
        tk.must_query_contain(
            "explain format = 'plan_tree' select /* issue:61134 */ * from t where a in ('')",
            &["Point_Get"],
        );
        tk.must_query_check("select /* issue:61134 */ * from t where a in ('')", &[" 1"]);

        for tc in char_null_safe_cases() {
            exercise_null_safe_equal_case(
                tk,
                "varchar(9)",
                &tc.partition_by,
                &[
                    ("'D'", tc.part_d),
                    ("'Y'", tc.part_y),
                    ("NULL", tc.part_null),
                ],
            );
        }

        for tc in int_null_safe_cases() {
            exercise_null_safe_equal_case(
                tk,
                "int",
                &tc.partition_by,
                &[("1", tc.part_1), ("5", tc.part_5), ("NULL", tc.part_null)],
            );
        }
    });
}

// exercise_null_safe_equal_case 对应 Go 中 testCaseChar/testCaseInt 的双循环。
// 先测 unique index 允许 NULL，再测 primary key 拒绝 NULL，最后检查 point/range/table dual 分支。
/// 对给定分区定义跑 `<=>`：unique 可空与主键非空两套。
pub fn exercise_null_safe_equal_case(
    tk: &mut TestKitDraft,
    column_type: &str,
    partition_by: &str,
    values_and_parts: &[(&str, &str)],
) {
    let seed_rows = if column_type == "int" {
        "insert into t values (1),(5),(NULL)"
    } else {
        "insert into t values ('Y'),('D'),(NULL)"
    };
    tk.must_exec("drop table if exists t");
    tk.must_exec(format!(
        "CREATE TABLE t (a {column_type}, unique index (a)) {partition_by}"
    ));
    tk.must_exec(seed_rows);
    for (value, part) in values_and_parts {
        let expected_value = if *value == "NULL" {
            "<nil>"
        } else {
            value.trim_matches('\'')
        };
        tk.must_query_check(
            format!("select /* issue:61176 */ a from t where a <=> {value}"),
            &[expected_value],
        );
        tk.must_query_contain(
            format!(
                "explain format='plan_tree' select /* issue:61176 */ a from t where a <=> {value}"
            ),
            &["partition:", part],
        );
    }

    tk.must_exec("drop table t");
    tk.must_exec(format!(
        "CREATE TABLE t (a {column_type} PRIMARY KEY) {partition_by}"
    ));
    tk.must_exec(seed_rows.replace(",(NULL)", ""));
    tk.must_contain_err_msg(
        "insert into t values (NULL)",
        "[table:1048]Column 'a' cannot be null",
    );
    for (value, part) in values_and_parts {
        tk.must_query_contain(
            format!(
                "explain format='plan_tree' select /* issue:61176 */ a from t where a <=> {value}"
            ),
            &["partition:", part],
        );
    }
    tk.must_exec("drop table t");
}

// test_cast 对应 Go 的 TestCast。
// 它保留两个带复杂字符集/分区定义的表、插入数据和最终 join 聚合断言。
#[test]
/// 对应 Go `TestCast`：复杂字符集分区表 join 聚合回归。
pub fn test_cast() {
    let _serial = serial_partition_pruner_test();
    run_test_under_cascades(|tk, cascades, _| {
        // Connection IDs keep repeated test invocations isolated from the
        // lightweight mock store's database cleanup implementation.
        let database = format!("test_cast_{cascades}_{}", tk.inner.ConnectionID());
        tk.must_exec(format!("drop database if exists {database}"));
        tk.must_exec(format!("create database {database}"));
        tk.must_exec(format!("use {database}"));
        tk.must_exec("drop table if exists t4365b15e, tf460485d");
        for sql in CAST_TABLE_AND_DATA_SQL {
            tk.must_exec(*sql);
        }
        let query = "SELECT SUM(t4365b15e.col_19) AS r0, tf460485d.col_60 FROM tf460485d JOIN t4365b15e ON tf460485d.col_60=t4365b15e.col_22 GROUP BY tf460485d.col_60 HAVING tf460485d.col_60 IN (1);";
        tk.must_query_check(query, &["53196 1"]);
    });
}

/// 对应 testfailpoint：强制开启 dynamic prune for the following test run.
pub fn enable_force_dynamic_prune() {
    std::mem::forget(enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    ));
}

/// 启用指定 failpoint until the matching disable call.
pub fn enable_failpoint(path: &str, action: &str) {
    std::mem::forget(enable(path, action));
}

/// 关闭指定 failpoint。
pub fn disable_failpoint(path: &str) {
    disable(path);
}

/// 对应 `testkit.RunTestUnderCascades`，使用真实 TestKit 运行两个 planner 模式。
pub fn run_test_under_cascades<F>(mut f: F)
where
    F: FnMut(&mut TestKitDraft, &str, &str),
{
    for (cascades, caller) in [("off", "classic"), ("on", "cascades")] {
        let mut tk = TestKitDraft::new();
        tk.must_exec(format!(
            "set @@tidb_enable_cascades_planner = {}",
            if cascades == "on" { "on" } else { "off" }
        ));
        f(&mut tk, cascades, caller);
    }
}

/// hash 分区裁剪 testdata 单条：SQL 与期望结果行。
pub struct PrunerCase {
    pub sql: String,
    pub expected: Vec<String>,
}

/// 对应 LoadTestCases：加载 partition_pruner suite 的真实 SQL/Result。
pub fn load_partition_pruner_cases(cascades: &str, _caller: &str, suite: &str) -> Vec<PrunerCase> {
    let data = testdata::LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "partition_pruner",
        cascades == "on",
    )
    .unwrap_or_else(|error| panic!("load partition_pruner: {error}"));
    let (input, output) = data
        .LoadTestCasesByName(suite, cascades == "on")
        .unwrap_or_else(|error| panic!("load {suite}: {error}"));
    input
        .as_array()
        .expect("pruner input array")
        .iter()
        .zip(output.as_array().expect("pruner output array"))
        .map(|(sql, output)| PrunerCase {
            sql: sql.as_str().unwrap().to_owned(),
            expected: output["Result"]
                .as_array()
                .expect("pruner result array")
                .iter()
                .map(|row| row.as_str().unwrap().to_owned())
                .collect(),
        })
        .collect()
}

/// list columns 裁剪用例：SQL、pruner 信息、结果与两套 plan。
pub struct ListColumnsCase {
    pub sql: String,
    pub pruner: String,
    pub result: Vec<String>,
    pub plan: Vec<String>,
    pub index_plan: Vec<String>,
}

/// 加载 list columns 裁剪用例的真实 plans、结果和 pruner 期望。
pub fn load_list_columns_cases(cascades: &str, _caller: &str) -> Vec<ListColumnsCase> {
    let data = testdata::LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "partition_pruner",
        cascades == "on",
    )
    .unwrap_or_else(|error| panic!("load partition_pruner: {error}"));
    let (input, output) = data
        .LoadTestCasesByName("TestListColumnsPartitionPruner", cascades == "on")
        .expect("list columns fixture");
    let standard_output = if cascades == "on" {
        let standard = testdata::LoadTestSuiteDataWithCascades(
            concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
            "partition_pruner",
            false,
        )
        .expect("standard list columns fixture");
        Some(
            standard
                .LoadTestCasesByName("TestListColumnsPartitionPruner", false)
                .expect("standard list columns cases")
                .1,
        )
    } else {
        None
    };
    input
        .as_array()
        .unwrap()
        .iter()
        .zip(output.as_array().unwrap())
        .enumerate()
        .map(|(index, (input, output))| {
            let result = output
                .get("Result")
                .and_then(|value| value.as_array())
                .or_else(|| {
                    standard_output.as_ref().and_then(|standard| {
                        standard.as_array()?.get(index)?.get("Result")?.as_array()
                    })
                })
                .map(|rows| {
                    rows.iter()
                        .map(|row| row.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            ListColumnsCase {
                sql: input["SQL"].as_str().unwrap().to_owned(),
                pruner: input["Pruner"].as_str().unwrap().to_owned(),
                result,
                plan: output["Plan"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row.as_str().unwrap().to_owned())
                    .collect(),
                index_plan: output["IndexPlan"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row.as_str().unwrap().to_owned())
                    .collect(),
            }
        })
        .collect()
}

/// Hash 分区裁剪测试的建表 DDL 矩阵。
pub const HASH_PARTITION_SETUP: &[&str] = &[
    "create table t2(id int, a int, b int, primary key(id, a)) partition by hash(id + a) partitions 10;",
    "create table t1(id int primary key, a int, b int) partition by hash(id) partitions 10;",
    "create table t3(id int, a int, b int, primary key(id, a)) partition by hash(id) partitions 10;",
    "create table t4(d datetime, a int, b int, primary key(d, a)) partition by hash(year(d)) partitions 10;",
    "create table t5(d date, a int, b int, primary key(d, a)) partition by hash(month(d)) partitions 10;",
    "create table t6(a int, b int) partition by hash(a) partitions 3;",
    "create table t7(a int, b int) partition by hash(a + b) partitions 10;",
    "create table t8(a int, b int) partition by hash(a) partitions 6;",
    "create table t9(a bit(1) default null, b int(11) default null) partition by hash(a) partitions 3;",
    "create table t10(a bigint unsigned) partition BY hash (a);",
    "create table t11(a int, b int) partition by hash(a + a + a + b) partitions 5",
];

/// List columns 裁剪测试的库表与数据准备 SQL。
pub const LIST_COLUMNS_PARTITION_SETUP: &[&str] = &[
    "drop database if exists test_partition;",
    "create database test_partition",
    "use test_partition",
    "create table t1 (id int, a int, b int) partition by list columns (b,a) (partition p0 values in ((1,1),(2,2),(3,3),(4,4),(5,5)), partition p1 values in ((6,6),(7,7),(8,8),(9,9),(10,10),(null,10)));",
    "create table t2 (id int, a int, b int) partition by list columns (id,a,b) (partition p0 values in ((1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5)), partition p1 values in ((6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,null,null)));",
    "insert into t1 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,10,null)",
    "insert into t2 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,null,null)",
    "drop database if exists test_partition_1;",
    "set @@session.tidb_regard_null_as_point=false",
    "create database test_partition_1",
    "use test_partition_1",
    "create table t1 (id int, a int, b int, unique key (a,b,id)) partition by list columns (b,a) (partition p0 values in ((1,1),(2,2),(3,3),(4,4),(5,5)), partition p1 values in ((6,6),(7,7),(8,8),(9,9),(10,10),(null,10)));",
    "create table t2 (id int, a int, b int, unique key (a,b,id)) partition by list columns (id,a,b) (partition p0 values in ((1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5)), partition p1 values in ((6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,null,null)));",
    "insert into t1 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,10,null)",
    "insert into t2 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,null,null)",
    "drop database if exists test_partition_2;",
    "set @@session.tidb_regard_null_as_point=false",
    "create database test_partition_2",
    "use test_partition_2",
    "create table t1 (id int, a int, b int)",
    "create table t2 (id int, a int, b int)",
    "insert into t1 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,10,null)",
    "insert into t2 (id,a,b) values (1,1,1),(2,2,2),(3,3,3),(4,4,4),(5,5,5),(6,6,6),(7,7,7),(8,8,8),(9,9,9),(10,10,10),(null,null,null)",
];

/// DATE/DATETIME 上各 time_unit 的 EXTRACT 裁剪期望集。
pub fn date_extract_cases<'a>() -> Vec<ExtractTestCase<'a>> {
    vec![
        ExtractTestCase {
            time_unit: "YEAR",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[
                "p2",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p2,p3",
            ],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "QUARTER",
            column_types: &["DATE", "DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "YEAR_MONTH",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[
                "p2",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p2,p3",
            ],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MONTH",
            column_types: &["DATE", "DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "WEEK",
            column_types: &[],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY",
            column_types: &["DATE", "DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_HOUR",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_MINUTE",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_SECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_MICROSECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR_MINUTE",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR_SECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR_MICROSECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "p1",
        },
        ExtractTestCase {
            time_unit: "MINUTE",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MINUTE_SECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MINUTE_MICROSECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "p1",
        },
        ExtractTestCase {
            time_unit: "SECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "SECOND_MICROSECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "p1",
        },
        ExtractTestCase {
            time_unit: "MICROSECOND",
            column_types: &["DATETIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "p1",
        },
    ]
}

/// TIME/TIMESTAMP 上各 time_unit 的 EXTRACT 裁剪期望集。
pub fn time_extract_cases<'a>() -> Vec<ExtractTestCase<'a>> {
    vec![
        ExtractTestCase {
            time_unit: "YEAR",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "QUARTER",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "YEAR_MONTH",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MONTH",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "WEEK",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY",
            column_types: &["DATE", "DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_HOUR",
            column_types: &["DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_MINUTE",
            column_types: &["DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_SECOND",
            column_types: &["DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "DAY_MICROSECOND",
            column_types: &["DATETIME"],
            prune_result: &[],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR",
            column_types: &["TIME"],
            prune_result: &[
                "p2",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p2,p3",
            ],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR_MINUTE",
            column_types: &["TIME"],
            prune_result: &[
                "p2",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p2,p3",
            ],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR_SECOND",
            column_types: &["TIME"],
            prune_result: &[
                "p2",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p2,p3",
            ],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "HOUR_MICROSECOND",
            column_types: &["TIME"],
            prune_result: &[
                "p2",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p0,p1,p2",
                "p2,p3,pMax",
                "p2,p3",
            ],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MINUTE",
            column_types: &["TIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MINUTE_SECOND",
            column_types: &["TIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MINUTE_MICROSECOND",
            column_types: &["TIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "SECOND",
            column_types: &["TIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "SECOND_MICROSECOND",
            column_types: &["TIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
        ExtractTestCase {
            time_unit: "MICROSECOND",
            column_types: &["TIME"],
            prune_result: &["p2", "all", "all", "all", "all", "all"],
            no_fsp_result: "",
        },
    ]
}

#[derive(Clone, Copy)]
/// 回归步骤种类：执行 SQL、校验行、或检查 explain 包含串。
pub enum RegressionStepKind {
    Exec,
    Rows,
    Contains,
}

/// issue 59827 回归步骤：Exec / Rows / Contains 之一。
pub struct RegressionStep<'a> {
    pub kind: RegressionStepKind,
    pub sql: &'a str,
    pub expected: &'a [&'a str],
}

/// issue 59827 相关逐步 SQL 与期望。
pub const REGRESSION_59827_STEPS: &[RegressionStep<'_>] = &[
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "drop table if exists t",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "CREATE TABLE `t` (`a` varchar(150) NOT NULL,`b` varchar(100) NOT NULL,`c` int NOT NULL DEFAULT '0',PRIMARY KEY (`a`,`b`) /*T![clustered_index] CLUSTERED */) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci PARTITION BY LIST COLUMNS(`b`)(PARTITION `p0` VALUES IN ('0'),PARTITION `p1` VALUES IN ('1'),PARTITION `p2` VALUES IN ('2'))",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "insert into t values ('a','1',1),('b','1',1),('b', '2', 2)",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "set @@session.tidb_partition_prune_mode = 'static'",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where a = 'b' and b IN('1','2')",
        expected: &["b 1 1", "b 2 2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where a = 'b' and (b = '2' or b = '1')",
        expected: &["b 1 1", "b 2 2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "set @@session.tidb_partition_prune_mode = 'dynamic'",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where a = 'b' and b = '2'",
        expected: &["b 2 2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where a = 'b' and b = ('1')",
        expected: &["b 1 1"],
    },
    RegressionStep {
        kind: RegressionStepKind::Contains,
        sql: "explain format = 'plan_tree' select /* issue:59827 */ * from t where a = 'b' and b = '2'",
        expected: &["partition:p2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "PREPARE stmt FROM 'select * from t where a = ? and b = ?'",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "SET @a = 'b', @b = '2'",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "EXECUTE stmt USING @a, @b",
        expected: &["b 2 2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "DEALLOCATE PREPARE stmt",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "drop table if exists t",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "CREATE TABLE `t` (`a` varchar(150) NOT NULL,`b` varchar(100) NOT NULL,`c` int NOT NULL DEFAULT '0',PRIMARY KEY (`b`) /*T![clustered_index] CLUSTERED */) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci PARTITION BY KEY(`b`) PARTITIONS 13",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "insert into t values ('a','3',3),('b','1',1),('b', '2', 2),('xX','xX',10),('Yy','Yy',11)",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where (b IN ('Xx','yY'))",
        expected: &["Yy Yy 11", "xX xX 10"],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where b = '2' or b = '1'",
        expected: &["b 1 1", "b 2 2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "drop table if exists t",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "CREATE TABLE `t` (`a` varchar(150) COLLATE utf8mb4_general_ci NOT NULL,`b` varchar(100) COLLATE utf8mb4_general_ci NOT NULL,`c` int NOT NULL DEFAULT '0',PRIMARY KEY (`a`,`b`) /*T![clustered_index] CLUSTERED */) PARTITION BY RANGE COLUMNS(`b`)(PARTITION `p0` VALUES LESS THAN ('1'),PARTITION `p1` VALUES LESS THAN ('2'),PARTITION `p2` VALUES LESS THAN ('3'))",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Exec,
        sql: "insert into t values ('a','1',1),('b','1',1),('b', '2', 2)",
        expected: &[],
    },
    RegressionStep {
        kind: RegressionStepKind::Rows,
        sql: "select /* issue:59827 */ * from t where a = 'b' and b = '2'",
        expected: &["b 2 2"],
    },
    RegressionStep {
        kind: RegressionStepKind::Contains,
        sql: "explain format = 'plan_tree' select /* issue:59827 */ * from t where a = 'a' and (b = '2')",
        expected: &["partition:p2"],
    },
];

/// 字符型列 `<=>` 在不同分区定义下的期望分区名。
pub struct CharNullSafeCase<'a> {
    pub partition_by: &'a str,
    pub part_d: &'a str,
    pub part_y: &'a str,
    pub part_null: &'a str,
}

/// 构造字符型 NULL-safe equal 分区用例集。
pub fn char_null_safe_cases<'a>() -> Vec<CharNullSafeCase<'a>> {
    vec![
        CharNullSafeCase {
            partition_by: "PARTITION BY RANGE COLUMNS (a) (PARTITION pNULL VALUES LESS THAN (''), PARTITION p0 VALUES LESS THAN ('M'), PARTITION p1 VALUES LESS THAN (MAXVALUE))",
            part_d: "p0",
            part_y: "p1",
            part_null: "pNULL",
        },
        CharNullSafeCase {
            partition_by: "PARTITION BY LIST COLUMNS (a) (PARTITION p0 VALUES IN ('D'), PARTITION p1 VALUES IN ('Y'), PARTITION pNULL VALUES IN (NULL))",
            part_d: "p0",
            part_y: "p1",
            part_null: "pNULL",
        },
        CharNullSafeCase {
            partition_by: "PARTITION BY KEY (a) PARTITIONS 2",
            part_d: "p0",
            part_y: "p1",
            part_null: "p1",
        },
    ]
}

/// 整型列 `<=>` 在不同分区定义下的期望分区名。
pub struct IntNullSafeCase<'a> {
    pub partition_by: &'a str,
    pub part_1: &'a str,
    pub part_5: &'a str,
    pub part_null: &'a str,
}

/// 构造整型 NULL-safe equal 分区用例集。
pub fn int_null_safe_cases<'a>() -> Vec<IntNullSafeCase<'a>> {
    vec![
        IntNullSafeCase {
            partition_by: "PARTITION BY RANGE (a) (PARTITION pNULL VALUES LESS THAN (0), PARTITION p0 VALUES LESS THAN (5), PARTITION p1 VALUES LESS THAN (MAXVALUE))",
            part_1: "p0",
            part_5: "p1",
            part_null: "pNULL",
        },
        IntNullSafeCase {
            partition_by: "PARTITION BY LIST (a) (PARTITION p0 VALUES IN (1), PARTITION p1 VALUES IN (5), PARTITION pNULL VALUES IN (NULL))",
            part_1: "p0",
            part_5: "p1",
            part_null: "pNULL",
        },
        IntNullSafeCase {
            partition_by: "PARTITION BY KEY (a) PARTITIONS 3",
            part_1: "p2",
            part_5: "p1",
            part_null: "p1",
        },
        IntNullSafeCase {
            partition_by: "PARTITION BY HASH (a) PARTITIONS 3",
            part_1: "p1",
            part_5: "p2",
            part_null: "p0",
        },
    ]
}

/// TestCast 所用复杂字符集分区表与插入数据 SQL。
pub const CAST_TABLE_AND_DATA_SQL: &[&str] = &[
    "CREATE TABLE t4365b15e (col_18 tinyblob DEFAULT NULL,col_19 smallint unsigned NOT NULL DEFAULT '14683',col_20 char(39) COLLATE utf8_bin DEFAULT '',col_21 time NOT NULL DEFAULT '01:08:07',col_22 char(153) COLLATE utf8_unicode_ci NOT NULL DEFAULT '-仂',UNIQUE KEY idx_11 (col_22,col_21) /*T![global_index] GLOBAL */,UNIQUE KEY idx_12 (col_22) /*T![global_index] GLOBAL */) PARTITION BY RANGE COLUMNS(col_22)(PARTITION p0 VALUES LESS THAN ('腅tD蠊KV吵d啗s'), PARTITION p1 VALUES LESS THAN (MAXVALUE))",
    "INSERT INTO t4365b15e VALUES(x'4a5955476d',32987,'uQKDBD99zce','09:35:47','y瘇=r賜莶)2g壅T敤q屉'),(x'4a5955476d',25115,'uQKDBD99zce','09:35:47','@j-yDA'),(x'406b6271',22117,'c^E@*ybYRTqNkAgTFC5','19:31:21','綑鞗')",
    "INSERT INTO t4365b15e VALUES(x'2d5e',4433,'d8G_Ic3Y*!rom**','09:38:32','1g0餪Pc诡Y&4y槄(Ou稠9嘪'),(x'69595e6467682b',48233,'nP)$V','14:32:31','埼dR鿱'),(x'6f5f2b5e7a3346235a64787954784975566f',57918,'o^8g*wCsmo@FBXmOS','09:35:45','鰻e=r+'),(x'4a5955476d',65016,'uQKDBD99zce','09:35:47','w层'),(x'6169262d4b243d792a4775',58947,'uQKDBD99zce','09:35:47','wqT8擟t獱Ip'),(x'6d6e6a40415e48395e423634',20706,'MMz1zdUDgztb@','22:11:26','rUN'),(x'4a517a6b3066716f426823',41270,'v67T4$LO7uy0','19:28:09','!鞤鏈鋎2p擛)nX4QI'),(x'36697124377042514a2521516c232862',23768,'c~~J','00:39:02','m'),(x'4a5955476d',61509,'uQKDBD99zce','09:35:47','鐩UQBb籹JQ57N韧S仂Fw'),(x'4d6f78586d6e4a68426e5e2d71',41755,'wICNk7+@HQ&j)&ojFL','20:10:51','0GIUbF賕#B'),(x'475a2444684c4a41',24780,'cPIPfD9Pd','20:30:43','YO5(e秬v鼾Bk扣竷6I韤$Zp'),(x'4a5955476d',51992,'uQKDBD99zce','09:35:47','N!SwEUsn堶I鐱'),(x'4a5955476d',65535,'uQKDBD99zce','09:35:47','Z5NL畏閻蚵-'),(x'7751514d',25824,'LO9TOd^CI_MtE7LR^z','22:42:26','@睾掘~坒cq(o6X鈞'),(x'38423771215333455e2d3842566b7641',49763,'yCyNZ$M8Q','12:52:23','嵌&)oW佞u稠9嘪'),(x'4a5955476d',65217,'uQKDBD99zce','09:35:47','%qnr'),(x'4a5955476d',37678,'uQKDBD99zce','09:35:47','陂c隊n杭S鶔瓱p5')",
    "CREATE TABLE tf460485d (col_57 tinytext COLLATE utf8_unicode_ci DEFAULT NULL,col_58 varchar(477) COLLATE utf8_general_ci NOT NULL DEFAULT '覽',col_59 tinyint unsigned NOT NULL,col_60 tinyint(1) NOT NULL DEFAULT '0',col_61 tinyint(1) NOT NULL,col_62 date DEFAULT '2031-12-15',col_63 smallint unsigned DEFAULT '28366',col_64 varbinary(389) NOT NULL,UNIQUE KEY idx_17 (col_58),PRIMARY KEY (col_58) /*T![clustered_index] NONCLUSTERED */ /*T![global_index] GLOBAL */) PARTITION BY RANGE COLUMNS(col_58)(PARTITION p0 VALUES LESS THAN ('$0Wc櫙'), PARTITION p1 VALUES LESS THAN ('襑蒢$堀$hKiV'), PARTITION p2 VALUES LESS THAN (MAXVALUE))",
    "INSERT INTO tf460485d VALUES('1','R獾',127,1,0,'1999-06-26',NULL,x'6f416e515e4e'),('-9Wd+h9FNWgtZ','~x',82,0,0,'2035-10-28',NULL,x''),('=I','UtEvX殃h瞉CpWOE',171,1,0,'1973-03-18',858,x'524228455f71'),('0ATpI@W+8uF','xq烱O',146,1,0,'1986-12-17',26667,x'76664d59257456233951366f506e'),('1','dT啛鋌PeJh喅AL',33,1,0,'2022-05-06',5881,x'296748734d6f7634525f655f26556a7161'),('1','Y掿麶絊磵夬$楯%悒也贒R',255,1,0,'2014-11-06',10258,x'6f6a2a624a414e445a686a32237a7a23536664'),('0ATpI@W+8uF','D7Xa',203,1,0,'1981-05-12',26055,x'695a45617445344c5a233872436d'),('1','-Jw瞲80z灧UsY觱艵X礋',156,1,0,'2007-09-27',31907,x'6b74'),('9Zv(^6PEL5%r','cm痵e缓R崻*駌Z*TuM',175,0,1,'2031-10-08',32717,x'6f462d634f4b795e58454a423246646b356666'),('0ATpI@W+8uF','(3攓i痈XDQ許WL薹F錓',119,0,1,'1981-02-19',56610,x'6c2163514c5026'),('cCfZwARDfmRnhT4V8D+','$4#Y助怔s',230,0,0,'2020-06-23',12937,x'5455554d744a5a75412a43365f41387a5456'),('s(O04','fQjXE#L櫘UOa膸',157,0,0,'2010-12-19',28999,x'26494a73216342313d344d514d'),('LR5^o$*2','跪熝葌0O冾湂~(D5犣Y@*',81,0,0,'2013-05-30',15558,x'7a78625e4c4f6b752d472854'),('6*Y$H62VB)','襾X_梼乵',25,0,1,'2024-03-29',40076,x'67576c505a21457a576b264f64523d5e4149'),('VH5P+UikL(','e坲V',31,1,1,'2005-05-07',1799,x'785e'),('0ATpI@W+8uF','%yr',61,0,1,'1976-07-20',65535,x'2d6566576a'),('!hJ7J!C#','d讦Il8鬏挻#R蜃顧',241,1,0,'1975-08-22',46070,x'37665f6f4f6d797a3243'),('DEhuX+rH(Tqnww','P溃挋%T',224,1,0,'2006-04-13',60626,x'5e2a712a4c234479436b796a455274525459'),('~6','1H罥詟蝮%n6',127,0,0,'2024-06-12',24793,x'3d3126474a5236284a'),('0ATpI@W+8uF','yyR寓卖=淙Q!',148,1,0,'1991-07-05',32767,x''),('0ATpI@W+8uF','澪',105,0,0,'1981-02-15',NULL,x'5f406767'),('0ATpI@W+8uF','9騞G眡gMJP+v+鋜襑鲯I',171,1,1,'1993-06-26',47054,x'74466e576550'),('tZ@aRBI8&','%酬dV遇5',81,0,0,'1975-09-16',48645,x'2366546f7573756b267832')",
];

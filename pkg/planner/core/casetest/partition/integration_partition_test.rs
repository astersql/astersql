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

//! 分区规划器集成测试。
//!
//! 测试通过真实的 Rust `TestKit` 执行 SQL，并从 Go 版测试数据中读取预期计划与结果；
//! 重点对照动态/静态分区裁剪，并复用 cascades 开关下的同一组用例。

use astersql_config_kerneltype::{IsClassic, IsNextGen};
use astersql_testkit::{TestKit, testdata};
use astersql_testkit_testfailpoint::enable;

#[derive(Debug)]
/// 从 `integration_partition_suite` 夹具解析出的单条分区测试用例。
pub struct IntegrationCase {
    /// 待执行或待解释的 SQL。
    pub sql: String,
    /// 动态分区裁剪模式下的预期执行计划。
    pub dynamic_plan: Vec<String>,
    /// 静态分区裁剪模式下的预期执行计划。
    pub static_plan: Vec<String>,
    /// 只在单一裁剪模式下校验的预期执行计划。
    pub plan: Vec<String>,
    /// 两种裁剪模式应共同满足的查询结果。
    pub result: Vec<String>,
}

/// 将夹具中以空格分隔的行还原成 `TestKit` 用于比较的二维结果。
fn rows(values: &[String]) -> Vec<Vec<String>> {
    values
        .iter()
        .map(|value| value.split(' ').map(str::to_owned).collect())
        .collect()
}

/// 与 Go `MustQuery(...).Check(...)` 一致，查询必须成功且计划必须精确匹配。
fn check_plan(tk: &TestKit, sql: &str, expected: &[String]) {
    tk.MustQuery(sql, Vec::new()).Check(rows(expected));
}

/// 核对查询结果；无 `ORDER BY` 的用例先排序，以保持 Go 测试语义。
fn check_result(tk: &TestKit, sql: &str, expected: &[String]) {
    let mut result = tk.MustQuery(sql, Vec::new());
    if !sql.to_ascii_lowercase().contains("order by") {
        result.Sort();
    }
    result.Check(rows(expected));
}

/// 对 LIST 与 LIST COLUMNS 表分别核对动态、静态裁剪计划。
pub fn test_list_partition_pruning(tk: &mut TestKit, cascades: &str, caller: &str) {
    tk.MustExec("create database list_partition_pruning", Vec::new());
    tk.MustExec("use list_partition_pruning", Vec::new());
    tk.MustExec("drop table if exists tlist", Vec::new());
    tk.MustExec(
        "create table tlist (a int, b int) partition by list (a) (
            partition p0 values in (0, 1, 2), partition p1 values in (3, 4, 5),
            partition p2 values in (6, 7, 8), partition p3 values in (9, 10, 11),
            partition p4 values in (-1))",
        Vec::new(),
    );
    tk.MustExec(
        "create table tcollist (a int, b int) partition by list columns(a) (
            partition p0 values in (0, 1, 2), partition p1 values in (3, 4, 5),
            partition p2 values in (6, 7, 8), partition p3 values in (9, 10, 11),
            partition p4 values in (-1))",
        Vec::new(),
    );
    tk.MustExec("analyze table tlist", Vec::new());
    tk.MustExec("analyze table tcollist", Vec::new());

    let suite = if IsNextGen() {
        "TestListPartitionPruningForNextGen"
    } else {
        "TestListPartitionPruning"
    };
    for case in load_integration_partition_cases(cascades, caller, suite) {
        tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
        check_plan(tk, &case.sql, &case.dynamic_plan);
        tk.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
        check_plan(tk, &case.sql, &case.static_plan);
    }
}

#[test]
/// classic 内核入口；next-gen 使用独立入口，避免同一用例重复执行。
pub fn test_list_partition_pruning_classic() {
    if IsNextGen() {
        return;
    }
    let _force = enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    );
    crate::support::run_test_under_cascades(test_list_partition_pruning);
}

#[test]
/// next-gen 内核入口，与 classic 入口共享相同的裁剪检查主体。
pub fn test_list_partition_pruning_for_next_gen() {
    if IsClassic() {
        return;
    }
    let _force = enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    );
    crate::support::run_test_under_cascades(test_list_partition_pruning);
}

#[test]
/// 核对哈希分区表在两种裁剪模式下的 EXPLAIN 计划。
pub fn test_partition_table_explain() {
    let _force = enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    );
    crate::support::run_test_under_cascades(|tk, cascades, caller| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("create table t (a int primary key, b int, key (b)) partition by hash(a) (partition P0, partition p1, partition P2)", Vec::new());
        tk.MustExec("create table t2 (a int, b int)", Vec::new());
        tk.MustExec("insert into t values (1,1),(2,2),(3,3)", Vec::new());
        tk.MustExec("insert into t2 values (1,1),(2,2),(3,3)", Vec::new());
        tk.MustExec("analyze table t, t2 all columns", Vec::new());
        for case in load_integration_partition_cases(cascades, caller, "TestPartitionTableExplain")
        {
            tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
            check_plan(tk, &case.sql, &case.dynamic_plan);
            tk.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
            check_plan(tk, &case.sql, &case.static_plan);
        }
    });
}

#[test]
/// 覆盖哈希、范围和列表分区上的批量点查，并确保两种裁剪模式结果一致。
pub fn test_batch_point_get_table_partition() {
    let _force = enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    );
    crate::support::run_test_under_cascades(|tk, cascades, caller| {
        tk.MustExec("use test", Vec::new());
        for sql in BATCH_POINT_GET_TABLE_SETUP {
            tk.MustExec(sql, Vec::new());
        }
        for case in
            load_integration_partition_cases(cascades, caller, "TestBatchPointGetTablePartition")
        {
            tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
            check_plan(tk, &explain(&case.sql), &case.dynamic_plan);
            check_result(tk, &case.sql, &case.result);
            tk.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
            check_plan(tk, &explain(&case.sql), &case.static_plan);
            check_result(tk, &case.sql, &case.result);
        }
    });
}

#[test]
/// 验证批量点查计划中的访问对象能准确标识命中的物理分区。
pub fn test_batch_point_get_partition_for_access_object() {
    let _force = enable(
        "github.com/pingcap/tidb/pkg/planner/core/forceDynamicPrune",
        "return(true)",
    );
    crate::support::run_test_under_cascades(|tk, cascades, caller| {
        tk.MustExec("use test", Vec::new());
        for sql in ACCESS_OBJECT_SETUP {
            tk.MustExec(sql, Vec::new());
        }
        tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
        for case in load_integration_partition_cases(
            cascades,
            caller,
            "TestBatchPointGetPartitionForAccessObject",
        ) {
            check_plan(tk, &case.sql, &case.plan);
        }
    });
}

#[test]
/// 回归 Issue 58475：分区表的虚拟生成列与强制索引组合应能正常规划。
pub fn test_generated_column_with_partition() {
    crate::support::run_test_under_cascades(|tk, _, _| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec("CREATE TABLE tp (id int, c1 int, c2 int GENERATED ALWAYS AS (c1) VIRTUAL, KEY idx (id)) PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (0), PARTITION p1 VALUES LESS THAN (10000))", Vec::new());
        tk.MustExec("INSERT INTO tp (id, c1) VALUES (0, 1)", Vec::new());
        tk.MustQuery("select /*+ FORCE_INDEX(tp, idx) */id from tp where c2 = 2 group by id having id in (0)", Vec::new());
    });
}

#[test]
/// 验证复杂谓词被化简为空结果后，无论是否通过 hint 强制静态裁剪都生成 `TableDual`。
pub fn test_partition_prune_with_predicate_simplification() {
    crate::support::run_test_under_cascades(|tk, _, _| {
        tk.MustExec("use test", Vec::new());
        tk.MustExec(PREDICATE_SIMPLIFICATION_TABLE, Vec::new());
        check_plan(
            tk,
            PREDICATE_SIMPLIFICATION_QUERY_WITH_HINT,
            &PREDICATE_SIMPLIFICATION_PLAN
                .iter()
                .map(|row| (*row).to_owned())
                .collect::<Vec<_>>(),
        );
        check_plan(
            tk,
            PREDICATE_SIMPLIFICATION_QUERY,
            &PREDICATE_SIMPLIFICATION_PLAN
                .iter()
                .map(|row| (*row).to_owned())
                .collect::<Vec<_>>(),
        );
    });
}

fn explain(sql: &str) -> String {
    format!("explain format = 'plan_tree' {sql}")
}

/// 按测试套件名称加载 SQL、动态/静态计划及可选结果，并保持输入输出按下标对应。
pub fn load_integration_partition_cases(
    cascades: &str,
    _caller: &str,
    suite: &str,
) -> Vec<IntegrationCase> {
    let data = testdata::LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "integration_partition_suite",
        cascades == "on",
    )
    .unwrap_or_else(|error| panic!("load integration_partition_suite: {error}"));
    let (input, output) = data
        .LoadTestCasesByName(suite, cascades == "on")
        .unwrap_or_else(|error| panic!("load {suite}: {error}"));
    let input = input.as_array().expect("integration input array");
    let output = output.as_array().expect("integration output array");
    assert_eq!(
        input.len(),
        output.len(),
        "integration fixture input/output length mismatch for {suite}"
    );
    input
        .iter()
        .zip(output)
        .map(|(sql, output)| IntegrationCase {
            sql: sql.as_str().expect("fixture SQL").to_owned(),
            dynamic_plan: output["DynamicPlan"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            static_plan: output["StaticPlan"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            plan: output["Plan"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            result: output["Result"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

#[test]
fn test_access_object_fixture_plan_is_loaded() {
    let cases = load_integration_partition_cases(
        "off",
        "TestBatchPointGetPartitionForAccessObject",
        "TestBatchPointGetPartitionForAccessObject",
    );

    assert!(
        !cases.is_empty(),
        "access-object fixture must contain cases"
    );
    assert!(
        cases.iter().all(|case| !case.plan.is_empty()),
        "every access-object fixture case must retain its Go Plan assertion"
    );
}

/// 批量点查用例的建表与数据准备，覆盖不同分区方式及聚簇/非聚簇主键。
pub const BATCH_POINT_GET_TABLE_SETUP: &[&str] = &[
    "create table thash1(a int, b int, primary key(a,b) nonclustered) partition by hash(b) partitions 2",
    "insert into thash1 values(1,1),(1,2),(2,1),(2,2)",
    "create table trange1(a int, b int, primary key(a,b) nonclustered) partition by range(b) (partition p0 values less than (2), partition p1 values less than maxvalue)",
    "insert into trange1 values(1,1),(1,2),(2,1),(2,2)",
    "create table tlist1(a int, b int, primary key(a,b) nonclustered) partition by list(b) (partition p0 values in (0, 1), partition p1 values in (2, 3))",
    "insert into tlist1 values(1,1),(1,2),(2,1),(2,2)",
    "create table thash2(a int, b int, primary key(a,b)) partition by hash(b) partitions 2",
    "insert into thash2 values(1,1),(1,2),(2,1),(2,2)",
    "create table trange2(a int, b int, primary key(a,b)) partition by range(b) (partition p0 values less than (2), partition p1 values less than maxvalue)",
    "insert into trange2 values(1,1),(1,2),(2,1),(2,2)",
    "create table tlist2(a int, b int, primary key(a,b)) partition by list(b) (partition p0 values in (0, 1), partition p1 values in (2, 3))",
    "insert into tlist2 values(1,1),(1,2),(2,1),(2,2)",
    "create table thash3(a int, b int, primary key(a)) partition by hash(a) partitions 2",
    "insert into thash3 values(1,0),(2,0),(3,0),(4,0)",
    "create table trange3(a int, b int, primary key(a)) partition by range(a) (partition p0 values less than (3), partition p1 values less than maxvalue)",
    "insert into trange3 values(1,0),(2,0),(3,0),(4,0)",
    "create table tlist3(a int, b int, primary key(a)) partition by list(a) (partition p0 values in (0, 1, 2), partition p1 values in (3, 4, 5))",
    "insert into tlist3 values(1,0),(2,0),(3,0),(4,0)",
    "create table issue45889(a int) partition by list(a) (partition p0 values in (0, 1), partition p1 values in (2, 3))",
    "insert into issue45889 values (0),(0),(1),(1),(2),(2),(3),(3)",
];

/// 访问对象用例的数据准备，覆盖单列及多列 LIST COLUMNS 分区键。
pub const ACCESS_OBJECT_SETUP: &[&str] = &[
    "create table t1(a int, b int, UNIQUE KEY (b)) PARTITION BY HASH(b) PARTITIONS 4",
    "insert into t1 values(1, 1), (2, 2), (3, 3), (4, 4)",
    "CREATE TABLE t2 (id int primary key, name_id int) PARTITION BY LIST(id) (partition p0 values IN (1, 2), partition p1 values IN (3, 4), partition p3 values IN (5))",
    "insert into t2 values(1, 1), (2, 2), (3, 3), (4, 4)",
    "CREATE TABLE t3 (id int primary key, name_id int) PARTITION BY LIST COLUMNS(id) (partition p0 values IN (1, 2), partition p1 values IN (3, 4), partition p3 values IN (5))",
    "insert into t3 values(1, 1), (2, 2), (3, 3), (4, 4)",
    "CREATE TABLE t4 (id int, name_id int, unique key(id, name_id)) PARTITION BY LIST COLUMNS(id, name_id) (partition p0 values IN ((1, 1),(2, 2)), partition p1 values IN ((3, 3),(4, 4)), partition p3 values IN ((5, 5)))",
    "insert into t4 values(1, 1), (2, 2), (3, 3), (4, 4)",
    "CREATE TABLE t5 (id int, name varchar(10), unique key(id, name)) PARTITION BY LIST COLUMNS(id, name) (partition p0 values IN ((1,'a'),(2,'b')), partition p1 values IN ((3,'c'),(4,'d')), partition p3 values IN ((5,'e')))",
    "insert into t5 values(1, 'a'), (2, 'b'), (3, 'c'), (4, 'd')",
];

pub const PREDICATE_SIMPLIFICATION_TABLE: &str = "CREATE TABLE tla842d94a (col_1 varchar(188) CHARACTER SET gbk COLLATE gbk_bin NOT NULL, col_2 double NOT NULL, PRIMARY KEY (col_1,col_2) /*T![clustered_index] NONCLUSTERED */, UNIQUE KEY idx_2 (col_1,col_2), UNIQUE KEY idx_3 (col_1,col_2), KEY idx_4 (col_1,col_2) /*T![global_index] GLOBAL */) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci PARTITION BY RANGE COLUMNS(col_1) (PARTITION p0 VALUES LESS THAN ('E恘l57'), PARTITION p1 VALUES LESS THAN ('MboOU0'), PARTITION p2 VALUES LESS THAN ('Q&h髑UDZ娻躸(襲!籂35'), PARTITION p3 VALUES LESS THAN ('f獟@'), PARTITION p4 VALUES LESS THAN ('~W噽纓'))";

pub const PREDICATE_SIMPLIFICATION_QUERY_WITH_HINT: &str = "explain format = 'plan_tree' SELECT /*+ set_var(tidb_partition_prune_mode=\"static\") */ 1, char(tla842d94a.col_2, tla842d94a.col_2 using utf8mb4) AS col_383, tla842d94a.col_2 AS col_384 FROM tla842d94a WHERE tla842d94a.col_1 IN ('與P)凥i5', 'AI禡=Ymm滕籔湾$IUKiF3撔') AND char(tla842d94a.col_2, tla842d94a.col_2 using utf8mb4) IN ('9eQ)6nzji', 'bF!pOc~') AND NOT (tla842d94a.col_2 <> 3496.9237290113774) ORDER BY char(tla842d94a.col_2, tla842d94a.col_2 using utf8mb4), tla842d94a.col_2";
pub const PREDICATE_SIMPLIFICATION_QUERY: &str = "explain format = 'plan_tree' SELECT 1, char(tla842d94a.col_2, tla842d94a.col_2 using utf8mb4) AS col_383, tla842d94a.col_2 AS col_384 FROM tla842d94a WHERE tla842d94a.col_1 IN ('與P)凥i5', 'AI禡=Ymm滕籔湾$IUKiF3撔') AND char(tla842d94a.col_2, tla842d94a.col_2 using utf8mb4) IN ('9eQ)6nzji', 'bF!pOc~') AND NOT (tla842d94a.col_2 <> 3496.9237290113774) ORDER BY char(tla842d94a.col_2, tla842d94a.col_2 using utf8mb4), tla842d94a.col_2";
pub const PREDICATE_SIMPLIFICATION_PLAN: &[&str] = &[
    "Projection root  Column, Column, test.tla842d94a.col_2",
    "└─Sort root  Column, test.tla842d94a.col_2",
    "  └─Projection root  Column, Column, test.tla842d94a.col_2, char_func(cast(test.tla842d94a.col_2, bigint(22) BINARY), cast(test.tla842d94a.col_2, bigint(22) BINARY), utf8mb4)->Column",
    "    └─Projection root  1->Column, char_func(cast(test.tla842d94a.col_2, bigint(22) BINARY), cast(test.tla842d94a.col_2, bigint(22) BINARY), utf8mb4)->Column, test.tla842d94a.col_2",
    "      └─TableDual root  rows:0",
];

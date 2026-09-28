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

// MPP 相关优化 golden、输入构造与会话变量真实驱动测试。
//
// 对应 Go `mpp_test.go`：用 `Domain::set_tiflash_replica_for_test` 发布真实
// TiFlash replica 元数据，再由 `TestKit` 执行相同 DDL/DML/analyze/会话变量
// 与 `TestMPPJoin`/`TestMPPExchangeSender` 的 `plan_tree` golden；其余 suite 由完整
// fixture 清单与定向输入副作断言覆盖。MPP 即大规模并行处理执行模式。

use astersql_domain::Domain;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::TestData;
use std::sync::Arc;

/// Go `mpp_test.go` 中由 integration_suite 预加载的全部测试入口及用例数。
const GO_MPP_SUITE_CASES: [(&str, usize); 23] = [
    ("TestMPPJoin", 20),
    ("TestMPPLeftSemiJoin", 13),
    ("TestMPPOuterJoinBuildSideForBroadcastJoin", 2),
    (
        "TestMPPOuterJoinBuildSideForShuffleJoinWithFixedBuildSide",
        2,
    ),
    ("TestMPPOuterJoinBuildSideForShuffleJoin", 2),
    ("TestMPPShuffledJoin", 16),
    ("TestMPPJoinWithCanNotFoundColumnInSchemaColumnsError", 3),
    ("TestJoinNotSupportedByTiFlash", 4),
    ("TestMPPWithHashExchangeUnderNewCollation", 6),
    ("TestMPPWithBroadcastExchangeUnderNewCollation", 2),
    ("TestMPPAvgRewrite", 1),
    ("TestPushDownProjectionForMPP", 15),
    ("TestPushDownSelectionForMPP", 2),
    ("TestMppUnionAll", 6),
    ("TestMppJoinDecimal", 8),
    ("TestMppJoinExchangeColumnPrune", 1),
    ("TestMppFineGrainedJoinAndAgg", 2),
    ("TestMppVersion", 9),
    ("TestPushDownAggForMPP", 24),
    ("TestMppAggTopNWithJoin", 13),
    ("TestRejectSortForMPP", 8),
    ("TestMPPJoinWithoutUselessExchange", 3),
    ("TestMPPJoinWithRemoveUselessExchange", 1),
];

fn suite_cases(data: &TestData, name: &str, cascades: bool) -> Vec<String> {
    let (input, output) = data
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name} cases: {error}"));
    let input = input
        .as_array()
        .unwrap_or_else(|| panic!("{name} input cases must be an array"));
    let output = output
        .as_array()
        .unwrap_or_else(|| panic!("{name} output cases must be an array"));
    assert_eq!(input.len(), output.len(), "{name} input/output case count");

    let mut sqls = Vec::with_capacity(input.len());
    for (index, (input, output)) in input.iter().zip(output).enumerate() {
        let sql = input
            .as_str()
            .unwrap_or_else(|| panic!("{name}[{index}] input SQL must be a string"))
            .to_owned();
        let expected_sql = output
            .get("SQL")
            .and_then(|value| value.as_str())
            .unwrap_or_else(|| panic!("{name}[{index}] output SQL must be a string"));
        // Go's four `TestJoinNotSupportedByTiFlash` output records retain the
        // original `brief` EXPLAIN text although the shared input fixture was
        // upgraded to `plan_tree`; the query body must otherwise stay exact.
        let canonical_expected_sql =
            expected_sql.replace("format = 'brief'", "format = 'plan_tree'");
        assert_eq!(
            canonical_expected_sql, sql,
            "{name}[{index}] input/output SQL"
        );
        if !sql.trim_start().to_ascii_lowercase().starts_with("set ") {
            assert!(
                output
                    .get("Plan")
                    .and_then(|value| value.as_array())
                    .is_some(),
                "{name}[{index}] query case must have a plan"
            );
        }
        sqls.push(sql);
    }
    sqls
}

fn assert_plan_case(tk: &TestKit, suite: &TestData, name: &str, index: usize, cascades: bool) {
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name}: {error}"));
    let sql = input
        .as_array()
        .and_then(|cases| cases.get(index))
        .and_then(|case| case.as_str())
        .unwrap_or_else(|| panic!("{name}[{index}] input SQL"));
    let expected = output
        .as_array()
        .and_then(|cases| cases.get(index))
        .and_then(|case| case.get("Plan"))
        .and_then(|plan| plan.as_array())
        .unwrap_or_else(|| panic!("{name}[{index}] golden Plan"))
        .iter()
        .map(|row| row.as_str().expect("golden plan row").to_owned())
        .collect::<Vec<_>>();
    let actual = tk
        .MustQuery(sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "{name}[{index}] SQL: {sql}");
}

/// 创建 mock store + Domain，并返回绑定其上的 TestKit。
fn new_testkit() -> (Arc<Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

fn prepare_mpp_join_fixture(domain: &Domain, tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table d1_t(d1_k int, value int)", Vec::new());
    tk.MustExec("insert into d1_t values(1,2),(2,3)", Vec::new());
    tk.MustExec("analyze table d1_t all columns", Vec::new());
    tk.MustExec(
        "create table d2_t(d2_k decimal(10,2), value int)",
        Vec::new(),
    );
    tk.MustExec("insert into d2_t values(10.11,2),(10.12,3)", Vec::new());
    tk.MustExec("analyze table d2_t all columns", Vec::new());
    tk.MustExec("create table d3_t(d3_k date, value int)", Vec::new());
    tk.MustExec(
        "insert into d3_t values(date'2010-01-01',2),(date'2010-01-02',3)",
        Vec::new(),
    );
    tk.MustExec("analyze table d3_t all columns", Vec::new());
    tk.MustExec("create table fact_t(d1_k int, d2_k decimal(10,2), d3_k date, col1 int, col2 int, col3 int)", Vec::new());
    tk.MustExec("insert into fact_t values(1,10.11,date'2010-01-01',1,2,3),(1,10.11,date'2010-01-02',1,2,3),(1,10.12,date'2010-01-01',1,2,3),(1,10.12,date'2010-01-02',1,2,3)", Vec::new());
    tk.MustExec("insert into fact_t values(2,10.11,date'2010-01-01',1,2,3),(2,10.11,date'2010-01-02',1,2,3),(2,10.12,date'2010-01-01',1,2,3),(2,10.12,date'2010-01-02',1,2,3)", Vec::new());
    tk.MustExec("analyze table fact_t all columns", Vec::new());
    domain
        .set_tiflash_replica_for_test("test", "d1_t", 1, true)
        .expect("set d1_t TiFlash replica");
    domain
        .set_tiflash_replica_for_test("test", "d2_t", 1, true)
        .expect("set d2_t TiFlash replica");
    domain
        .set_tiflash_replica_for_test("test", "d3_t", 1, true)
        .expect("set d3_t TiFlash replica");
    domain
        .set_tiflash_replica_for_test("test", "fact_t", 1, true)
        .expect("set fact_t TiFlash replica");
    tk.MustExec("set @@session.tidb_allow_mpp=1", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines='tiflash'",
        Vec::new(),
    );
}

#[test]
fn test_mpp_join_plan_golden_is_executed() {
    let suite = super::main_test::load_integration_suite();
    let (domain, mut tk) = new_testkit();
    prepare_mpp_join_fixture(&domain, &mut tk);
    tk.MustExec("set @@session.tidb_enable_cascades_planner = 1", Vec::new());
    let mut failures = Vec::new();
    for index in 0..20 {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert_plan_case(&tk, &suite, "TestMPPJoin", index, true))).is_err() {
            failures.push(index);
        }
    }
    assert!(failures.is_empty(), "mismatched cascades golden indices: {failures:?}");
}

#[test]
fn scalar_mpp_tidb_aggregation_keeps_root_boundary() {
    let suite = super::main_test::load_integration_suite();
    let (domain, mut tk) = new_testkit();
    prepare_mpp_join_fixture(&domain, &mut tk);
    tk.MustExec("set @@session.tidb_enable_cascades_planner = 0", Vec::new());
    let (input, _) = suite
        .LoadTestCasesByName("TestMPPJoin", false)
        .expect("load Go MPP join cases");
    let sql = input.as_array().expect("cases array")[0]
        .as_str()
        .expect("first SQL");
    let plan = tk
        .MustQuery(sql, Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    assert!(
        plan.first()
            .is_some_and(|row| row.starts_with("StreamAgg root")),
        "scalar final aggregate must execute in TiDB: {plan:#?}"
    );
    assert!(
        plan.iter().any(|row| row.contains("TableReader root")),
        "MPP input must cross into TiDB: {plan:#?}"
    );
    assert!(
        plan.iter()
            .any(|row| row.contains("ExchangeSender mpp[tiflash]")),
        "MPP join must remain below the root boundary: {plan:#?}"
    );
    assert!(
        !plan.iter().any(|row| row.contains("HashAgg mpp[tiflash]")),
        "scalar final aggregate must not be pushed into MPP: {plan:#?}"
    );
    assert_plan_case(&tk, &suite, "TestMPPJoin", 0, false);
}

#[test]
fn nested_index_join_preserves_intermediate_lookup_keys() {
    let (domain, mut tk) = new_testkit();
    prepare_mpp_join_fixture(&domain, &mut tk);
    for cascades in [false, true] {
        tk.MustExec(
            &format!("set @@session.tidb_enable_cascades_planner = {}", u8::from(cascades)),
            Vec::new(),
        );
        let rows = tk.MustQuery(
            "select count(*) from fact_t, d1_t, d2_t, d3_t where fact_t.d1_k = d1_t.d1_k and fact_t.d2_k = d2_t.d2_k and fact_t.d3_k = d3_t.d3_k",
            Vec::new(),
        ).Rows();
        assert_eq!(rows, vec![vec!["8".to_owned()]]);
        let residual_sql = "select count(*) from fact_t, d1_t, d2_t, d3_t where fact_t.d1_k = d1_t.d1_k and fact_t.d2_k = d2_t.d2_k and fact_t.d3_k = d3_t.d3_k and fact_t.col2 = d2_t.value";
        assert_eq!(
            tk.MustQuery(residual_sql, Vec::new()).Rows(),
            vec![vec!["2".to_owned()]],
        );
        tk.MustQuery(&format!("explain format = 'plan_tree' {residual_sql}"), Vec::new());
    }
}

#[test]
fn rewritten_semi_join_preserves_null_rejecting_filters() {
    let suite = super::main_test::load_integration_suite();
    let (domain, mut tk) = new_testkit();
    prepare_mpp_join_fixture(&domain, &mut tk);
    for cascades in [false, true] {
        tk.MustExec(
            &format!("set @@session.tidb_enable_cascades_planner = {}", u8::from(cascades)),
            Vec::new(),
        );
        assert_plan_case(&tk, &suite, "TestMPPJoin", 12, cascades);
        assert_eq!(
            tk.MustQuery(
                "select count(*) from fact_t where exists (select /*+ SEMI_JOIN_REWRITE() */ 1 from d1_t where d1_k = fact_t.d1_k)",
                Vec::new(),
            )
            .Rows(),
            vec![vec!["8".to_owned()]],
        );
    }
}

#[test]
fn test_mpp_join_legacy_plan_golden_is_executed() {
    let suite = super::main_test::load_integration_suite();
    let (domain, mut tk) = new_testkit();
    prepare_mpp_join_fixture(&domain, &mut tk);
    tk.MustExec("set @@session.tidb_enable_cascades_planner = 0", Vec::new());
    let mut failures = Vec::new();
    for index in 0..20 {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert_plan_case(&tk, &suite, "TestMPPJoin", index, false))).is_err() {
            failures.push(index);
        }
    }
    assert!(failures.is_empty(), "mismatched legacy golden indices: {failures:?}");
}

/// Go `TestMPPExchangeSender`：两种 planner 模式都须产生相同的 PassThrough MPP 计划。
#[test]
fn test_mpp_exchange_sender_plan_matches_go() {
    let expected = vec![
        "Limit root  offset:0, count:100",
        "└─TableReader root  MppVersion: 3, data:ExchangeSender",
        "  └─ExchangeSender mpp[tiflash]  ExchangeType: PassThrough",
        "    └─Selection mpp[tiflash]  gt(plus(test.t.a, 1), 20)",
        "      └─TableFullScan mpp[tiflash] table:t keep order:false, stats:pseudo",
    ];
    for cascades in [false, true] {
        let (domain, mut tk) = new_testkit();
        tk.MustExec("use test", Vec::new());
        tk.MustExec("create table t(a int)", Vec::new());
        domain
            .set_tiflash_replica_for_test("test", "t", 1, true)
            .expect("set t TiFlash replica");
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner = {}",
                u8::from(cascades)
            ),
            Vec::new(),
        );
        let actual = tk
            .MustQuery(
                "explain format = 'plan_tree' select /* issue:36194 */ /*+ read_from_storage(tiflash[t]) */ * from t where a + 1 > 20 limit 100",
                Vec::new(),
            )
            .Rows()
            .into_iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "cascades={cascades}");
    }
}

/// Go `TestMain` 加载的 suite 必须保留全部 23 个入口、标准/XUT 两套输出和逐条 SQL 对齐。
///
/// `TestMPPJoin` 的 golden 另由真实 planner 执行；这里确保迁移没有删掉
/// 任何 Go fixture 场景、错配输入/输出，或遗失 standard/Cascades 任一输出。
#[test]
fn test_mpp_integration_suite_inventory_matches_go() {
    let suite = super::main_test::load_integration_suite();
    for (name, expected_count) in GO_MPP_SUITE_CASES {
        let standard = suite_cases(&suite, name, false);
        assert_eq!(standard.len(), expected_count, "{name} standard case count");

        let cascades = suite_cases(&suite, name, true);
        assert_eq!(cascades.len(), expected_count, "{name} Cascades case count");
    }
}

/// 从 Domain 统计句柄读取指定表的缓存 TableStats（表级统计元信息）。
fn stats_meta(domain: &Domain, table: &str) -> astersql_statistics_handle::TableStats {
    let table_id = domain
        .table_by_name("test", table)
        .unwrap_or_else(|error| panic!("test.{table} metadata: {error}"))
        .ID;
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(table_id)
        .unwrap_or_else(|| panic!("cached statistics for test.{table}"))
        .clone()
}

// test_mpp_join_fixture_tables_get_real_stats_after_analyze 对应 Go TestMPPJoin 的
// d1_t/d3_t analyze 阶段。完整 d1_t/d2_t/d3_t/fact_t（含 decimal 多批 DML）
// 已由 golden 测试覆盖；这里额外对行数统计这一副作用做精确断言。
/// analyze 后维度表须有真实（非 pseudo）统计，且 realtime_count 与插入行数一致。
#[test]
fn test_mpp_join_fixture_tables_get_real_stats_after_analyze() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table d1_t(d1_k int, value int)", Vec::new());
    tk.MustExec("insert into d1_t values(1,2),(2,3)", Vec::new());
    tk.MustExec("analyze table d1_t all columns", Vec::new());

    tk.MustExec("create table d3_t(d3_k date, value int)", Vec::new());
    tk.MustExec(
        "insert into d3_t values('2010-01-01',2),('2010-01-02',3)",
        Vec::new(),
    );
    tk.MustExec("analyze table d3_t all columns", Vec::new());

    for (table, rows) in [("d1_t", 2), ("d3_t", 2)] {
        let stats = stats_meta(&domain, table);
        assert!(
            !stats.pseudo,
            "{table} should have real stats after analyze"
        );
        assert_eq!(
            stats.realtime_count, rows,
            "{table} row count after analyze"
        );
    }
}

// test_mpp_outer_join_build_side_fixture_tables 对应 Go
// TestMPPOuterJoinBuildSideForBroadcastJoin/ForShuffleJoin(WithFixedBuildSide) 共用的建表
// 阶段：两张表 a/b，行数分别是 2 和 3，analyze 之后统计信息必须反映这个不对称的行数——这正是
// Go 测试想要驱动"选哪一侧做 build 端"这个 MPP 决策的统计输入。
/// outer join build 侧：a/b 行数 2/3 的不对称统计必须真实落盘。
#[test]
fn test_mpp_outer_join_build_side_fixture_tables() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table a(id int, value int)", Vec::new());
    tk.MustExec("insert into a values(1,2),(2,3)", Vec::new());
    tk.MustExec("analyze table a all columns", Vec::new());
    tk.MustExec("create table b(id int, value int)", Vec::new());
    tk.MustExec("insert into b values(1,2),(2,3),(3,4)", Vec::new());
    tk.MustExec("analyze table b all columns", Vec::new());

    assert_eq!(stats_meta(&domain, "a").realtime_count, 2);
    assert_eq!(stats_meta(&domain, "b").realtime_count, 3);
}

// test_mpp_new_collation_tables_keep_distinct_collations 对应 Go
// TestMPPWithHashExchangeUnderNewCollation/TestMPPWithBroadcastExchangeUnderNewCollation：
// table_1 显式用 utf8mb4_general_ci，table_2 用 utf8mb4_bin；这两种排序规则的差异正是 Go
// 测试想要驱动的"新排序规则下要不要为 hash exchange 补类型转换"场景，DDL 必须真实保留这个
// 差异，而不是被某种规范化悄悄合并成同一种排序规则。
/// 新排序规则下两表 value 列须分别保留 general_ci 与 bin，不可被合并。
#[test]
fn test_mpp_new_collation_tables_keep_distinct_collations() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table table_1(id int not null, value char(10), index idx(id, value)) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci",
        Vec::new(),
    );
    tk.MustExec("insert into table_1 values(1,'1'),(2,'2')", Vec::new());
    tk.MustExec(
        "create table table_2(id int not null, value char(10), index idx(id, value)) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin",
        Vec::new(),
    );
    tk.MustExec("insert into table_2 values(1,'1'),(2,'2')", Vec::new());

    let table_1 = domain
        .table_by_name("test", "table_1")
        .expect("test.table_1 metadata");
    let table_2 = domain
        .table_by_name("test", "table_2")
        .expect("test.table_2 metadata");
    let value_collate = |table: &astersql_meta_model::TableInfo| {
        table
            .Columns
            .iter()
            .find(|column| column.Name.L == "value")
            .expect("value column")
            .FieldType
            .GetCollate()
            .to_string()
    };
    assert_eq!(value_collate(&table_1), "utf8mb4_general_ci");
    assert_eq!(value_collate(&table_2), "utf8mb4_bin");
    assert_ne!(value_collate(&table_1), value_collate(&table_2));
}

// test_mpp_join_decimal_tables_preserve_precision_and_scale 对应 Go TestMppJoinDecimal：
// 建表语句里给出了好几组不同精度/标度的 decimal 列（c1..c5、col_decimal_30_10_key），这些
// 精度/标度差异正是 Go 测试想验证 MPP join 在做 decimal 类型对齐/提升时的输入，DDL 解析出的
// FieldType 必须逐列保留 Go 源码里写的 (flen, decimal)。
/// decimal 列的 flen/decimal（精度/标度）须与 DDL 声明逐列一致。
#[test]
fn test_mpp_join_decimal_tables_preserve_precision_and_scale() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t (c1 decimal(8, 5), c2 decimal(9, 5), c3 decimal(9, 4) NOT NULL, c4 decimal(8, 4) NOT NULL, c5 decimal(40, 20))",
        Vec::new(),
    );
    tk.MustExec(
        "create table tt (pk int(11) NOT NULL AUTO_INCREMENT primary key,col_varchar_64 varchar(64),col_char_64_not_null char(64) NOT null, col_decimal_30_10_key decimal(30,10), col_tinyint tinyint, col_varchar_key varchar(1), key col_decimal_30_10_key (col_decimal_30_10_key), key col_varchar_key(col_varchar_key))",
        Vec::new(),
    );

    let t = domain.table_by_name("test", "t").expect("test.t metadata");
    let column = |name: &str| {
        t.Columns
            .iter()
            .find(|column| column.Name.L == name)
            .unwrap_or_else(|| panic!("column {name}"))
            .clone()
    };
    for (name, flen, decimal) in [
        ("c1", 8, 5),
        ("c2", 9, 5),
        ("c3", 9, 4),
        ("c4", 8, 4),
        ("c5", 40, 20),
    ] {
        let field_type = &column(name).FieldType;
        assert_eq!(field_type.GetFlen(), flen, "{name} flen");
        assert_eq!(field_type.GetDecimal(), decimal, "{name} decimal");
    }

    let tt = domain
        .table_by_name("test", "tt")
        .expect("test.tt metadata");
    let key_column = tt
        .Columns
        .iter()
        .find(|column| column.Name.L == "col_decimal_30_10_key")
        .expect("col_decimal_30_10_key column");
    assert_eq!(key_column.FieldType.GetFlen(), 30);
    assert_eq!(key_column.FieldType.GetDecimal(), 10);
}

// test_mpp_join_exchange_column_prune_fixture_tables 对应 Go
// TestMppJoinExchangeColumnPrune/TestMppFineGrainedJoinAndAgg 共用的建表阶段：一张宽表 t
// （5 列）和一张单列窄表 tt，这组不对称的列数正是 Go 测试想驱动 exchange 阶段列裁剪的输入。
/// exchange 列裁剪输入：宽表 5 列 + 窄表 1 列，且 analyze 后非 pseudo。
#[test]
fn test_mpp_join_exchange_column_prune_fixture_tables() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t (c1 int, c2 int, c3 int NOT NULL, c4 int NOT NULL, c5 int)",
        Vec::new(),
    );
    tk.MustExec("create table tt (b1 int)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("analyze table tt", Vec::new());

    let t = domain.table_by_name("test", "t").expect("test.t metadata");
    let tt = domain
        .table_by_name("test", "tt")
        .expect("test.tt metadata");
    assert_eq!(t.Columns.len(), 5);
    assert_eq!(tt.Columns.len(), 1);
    assert!(!stats_meta(&domain, "t").pseudo);
    assert!(!stats_meta(&domain, "tt").pseudo);
}

// test_mpp_join_with_remove_useless_exchange_fixture_tables 对应 Go
// TestMPPJoinWithRemoveUselessExchange：四张单列主键表 t1..t4，都以 v1 为聚簇主键——这组
// "同分布" 表正是 Go 测试想验证"两侧已经按同一列分布时可以去掉多余 exchange" 的输入前提。
/// 同分布主键表 t1..t4：PKIsHandle 且列数为 2，作为去掉多余 exchange 的前提。
#[test]
fn test_mpp_join_with_remove_useless_exchange_fixture_tables() {
    let (domain, mut tk) = new_testkit();
    for table in ["t1", "t2", "t3", "t4"] {
        tk.MustExec(
            &format!("CREATE TABLE {table} (v1 INT NOT NULL, v2 INT NOT NULL, PRIMARY KEY (v1))"),
            Vec::new(),
        );
    }
    for table in ["t1", "t2", "t3", "t4"] {
        let info = domain
            .table_by_name("test", table)
            .unwrap_or_else(|error| panic!("test.{table} metadata: {error}"));
        assert!(info.PKIsHandle, "{table} has a single int primary key");
        assert_eq!(info.Columns.len(), 2);
    }
}

// test_mpp_session_variables_accept_real_set_statements 对应散布在几乎每个 TestMPP* 用例
// 里、用来在跑 golden 用例之前固定 MPP/broadcast join 相关会话状态的
// `set @@session.xxx`/`set @@xxx` 语句：MustExec 遇错会 panic，因此跑到最后一行即说明这批
// 变量全部被真实的 Set 执行链路（parser -> resolver -> Set executor -> vardef 校验）接受。
/// MPP / broadcast join 相关会话变量须被真实 SET 执行链路全部接受。
#[test]
fn test_mpp_session_variables_accept_real_set_statements() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 1", Vec::new());
    tk.MustExec("set @@session.tidb_enforce_mpp = 1", Vec::new());
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_size = 0",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_count = 0",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_opt_mpp_outer_join_fixed_build_side = 1",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_hash_exchange_with_new_collation = 1",
        Vec::new(),
    );
    tk.MustExec(
        "set @@tidb_isolation_read_engines='tiflash,tidb'",
        Vec::new(),
    );
}

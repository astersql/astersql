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

// 强制 MPP（Massively Parallel Processing，大规模并行处理）相关规划器用例。
//
// MPP 模式下 TiDB 将算子下推到 TiFlash（列存引擎）并行执行。`tidb_enforce_mpp`
// 强制走 MPP；会话还需 `tidb_allow_mpp` 等开关。Go 原版对照 enforce_mpp_suite
// 黄金 plan/warning；此处用真实建表/变量/归一化 helper/TiFlashReplica 形状等
// 生产 API 覆盖各 Go 用例中独立于完整 MPP 物理计划缺口的部分。

// 本文件对应 pkg/planner/core/casetest/enforcempp/enforce_mpp_test.go。Go 版本每个用例都是：
// 建表、伪造 TiFlashReplica、设 enforce/allow MPP 相关会话变量，再从 enforce_mpp_suite
// 读 SQL，跑 explain 与黄金 plan/warning 比对。
//
// Rust Domain 已提供 `set_tiflash_replica_for_test`，可以用真实元数据发布路径
// 复刻 Go `testkit.SetTiFlashReplica`。测试不伪造 plan/golden：优先直接执行
// suite SQL 并对比 Go golden，其余用例也用已编译生产 API 驱动真实建表、
// insert/analyze、session 变量、plan-id 归一化、warning 过滤和 new-collation 开关。

#![allow(non_snake_case)]

use astersql_meta_model::TiFlashReplicaInfo;
use astersql_sessionctx_vardef::{
    DefTiDBAllowMPPExecution, DefTiDBAllowTiFlashCop, DefTiDBEnforceMPPExecution,
    TiDBAllowTiFlashCop, TiDBEnableChunkRPC, TiDBEnforceMPPExecution, TiDBHashJoinVersion,
    TiDBIsolationReadEngines, TiDBOptEnable3StageMultiDistinctAgg,
};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::{ConvertRowsToStrings, TestData};
use astersql_util_collate::SetNewCollationEnabledForTest;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// 创建 mock store/domain 与 TestKit。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

// shouldNormalizeEnforceMPPPlan 对应 Go 同名函数。
/// 仅对 explain/desc 语句做算子 id 归一化后再比较计划行。
fn should_normalize_enforce_mpp_plan(sql: &str) -> bool {
    let s = sql.trim().to_ascii_lowercase();
    s.starts_with("explain") || s.starts_with("desc")
}

// normalize_plan_node_ids 对应 regexp.MustCompile(`_[0-9]+`).ReplaceAllString(row, "_N")。
/// 将计划行中 `_123` 形式的算子编号替换为 `_N`，便于跨运行比较。
fn normalize_plan_node_ids(row: &str) -> String {
    let mut out = String::with_capacity(row.len());
    let mut chars = row.chars().peekable();
    while let Some(ch) = chars.next() {
        // 匹配下划线后跟一段十进制数字，整段替换为 `_N`。
        if ch == '_' && chars.peek().is_some_and(char::is_ascii_digit) {
            out.push_str("_N");
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// 对多行计划统一做算子 id 归一化。
fn normalize_enforce_mpp_plan_rows(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| normalize_plan_node_ids(row))
        .collect()
}

/// 按 SQL 类型选择是否归一化后再比较期望与实际计划行。
fn equal_enforce_mpp_plan_rows(expected: &[String], actual: &[String], sql: &str) -> bool {
    if should_normalize_enforce_mpp_plan(sql) {
        normalize_enforce_mpp_plan_rows(expected) == normalize_enforce_mpp_plan_rows(actual)
    } else {
        expected == actual
    }
}

// filter_skyline_pruning_warnings 对应 TestEnforceMPP 里过滤
// "remain after pruning paths for" 的 warning 过滤器。
/// 过滤 skyline 路径剪枝提示，保留其余 warning（如 MPP 阻塞原因）。
fn filter_skyline_pruning_warnings(warnings: &[String]) -> Vec<String> {
    warnings
        .iter()
        .filter(|w| !w.contains("remain after pruning paths for"))
        .cloned()
        .collect()
}

/// 构造虚拟 TiFlash 副本元信息（副本数与 Available 标志）。
fn virtual_tiflash_replica(available: bool) -> TiFlashReplicaInfo {
    TiFlashReplicaInfo {
        Count: 1,
        Available: available,
        ..TiFlashReplicaInfo::default()
    }
}

/// Go `LoadTestCases` 的 Rust 侧只读视图，用于保留每条 suite SQL 与 golden plan。
struct EnforceMPPCase {
    sql: String,
    plan: Option<Vec<String>>,
    warnings: Vec<String>,
}

fn suite_cases(data: &TestData, name: &str, cascades: bool) -> Vec<EnforceMPPCase> {
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

    input
        .iter()
        .zip(output)
        .enumerate()
        .map(|(index, (sql, expected))| {
            let sql = sql
                .as_str()
                .unwrap_or_else(|| panic!("{name}[{index}] SQL must be a string"))
                .to_owned();
            let expected_sql = expected
                .get("SQL")
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| panic!("{name}[{index}] output SQL must be a string"));
            assert_eq!(expected_sql, sql, "{name}[{index}] SQL mismatch");
            let plan = expected.get("Plan").and_then(|value| {
                value.as_array().map(|rows| {
                    rows.iter()
                        .map(|row| {
                            row.as_str()
                                .unwrap_or_else(|| {
                                    panic!("{name}[{index}] plan row must be a string")
                                })
                                .to_owned()
                        })
                        .collect()
                })
            });
            let warnings = expected
                .get("Warn")
                .and_then(|value| value.as_array())
                .map(|warnings| {
                    warnings
                        .iter()
                        .map(|warning| {
                            warning
                                .as_str()
                                .unwrap_or_else(|| {
                                    panic!("{name}[{index}] warning must be a string")
                                })
                                .to_owned()
                        })
                        .collect()
                })
                .unwrap_or_default();
            EnforceMPPCase {
                sql,
                plan,
                warnings,
            }
        })
        .collect()
}

fn statement_warnings(tk: &TestKit) -> Vec<String> {
    tk.MustQuery("show warnings", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| {
            row.last()
                .unwrap_or_else(|| panic!("SHOW WARNINGS row must contain a message"))
                .clone()
        })
        .collect()
}

fn assert_golden_plan_and_warnings(tk: &TestKit, case: &EnforceMPPCase) {
    let expected = case
        .plan
        .as_ref()
        .unwrap_or_else(|| panic!("query case has no plan: {}", case.sql));
    let deadline = Instant::now() + Duration::from_secs(5);
    let actual = loop {
        let actual = ConvertRowsToStrings(&tk.MustQuery(&case.sql, Vec::new()).Rows());
        if equal_enforce_mpp_plan_rows(expected, &actual, &case.sql) {
            break actual;
        }
        if Instant::now() >= deadline {
            panic!(
                "sql: {}\nexpected: {expected:?}\nactual: {actual:?}",
                case.sql
            );
        }
        thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(actual.len(), expected.len(), "sql: {}", case.sql);

    let warnings = filter_skyline_pruning_warnings(&statement_warnings(tk));
    assert_eq!(case.warnings, warnings, "sql: {}", case.sql);
}

/// The Go TestMain loads 13 named suites and their Cascades output; the
/// standalone read-committed test has no suite entry. Keep the
/// same inventory check in Rust so a missing/renamed fixture cannot silently
/// remove coverage.
#[test]
fn test_enforce_mpp_suite_inventory_matches_go() {
    let data = super::main_test::load_enforce_mpp_suite_data();
    for (name, expected_count) in [
        ("TestEnforceMPP", 23),
        ("TestEnforceMPPWarning1", 16),
        ("TestEnforceMPPWarning2", 4),
        ("TestEnforceMPPWarning3", 4),
        ("TestEnforceMPPWarning4", 17),
        ("TestMPP2PhaseAggPushDown", 5),
        ("TestMPPSkewedGroupDistinctRewrite", 14),
        ("TestMPPSingleDistinct3Stage", 14),
        ("TestMPPMultiDistinct3Stage", 21),
        ("TestMPPNullAwareSemiJoinPushDown", 11),
        ("TestMPPSharedCTEScan", 17),
        ("TestRollupMPP", 16),
        ("TestEnforceMPPNewest", 2),
    ] {
        let cases = suite_cases(&data, name, false);
        assert_eq!(cases.len(), expected_count, "{name} standard case count");
        // TestMPPSharedCTEScan calls LoadTestCases without the cascades flag in
        // Go, so its historical xut file is not part of that test's contract.
        if name != "TestMPPSharedCTEScan" {
            let cascades_cases = suite_cases(&data, name, true);
            assert_eq!(
                cascades_cases.len(),
                expected_count,
                "{name} Cascades case count"
            );
        }
    }
}

/// The first three cases are ordinary session queries and should use the Go
/// golden values before any MPP-specific physical-plan capability is involved.
#[test]
fn test_enforce_mpp_session_defaults_match_golden() {
    let data = super::main_test::load_enforce_mpp_suite_data();
    let cases = suite_cases(&data, "TestEnforceMPP", false);
    let (_domain, tk) = new_testkit();
    for case in cases.iter().take(3) {
        assert_golden_plan_and_warnings(&tk, case);
    }
}

/// Go TestEnforceMPP 的标准/Cascades 路径：相同 DDL、TiFlash 副本、
/// session 命令、对应 golden plan/warning 和 Eventually 时序。
#[test]
fn test_enforce_mpp_standard_golden_matches_go() {
    let data = super::main_test::load_enforce_mpp_suite_data();
    for cascades in [false, true] {
        let cases = suite_cases(&data, "TestEnforceMPP", cascades);
        let (domain, mut tk) = new_testkit();
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner={}",
                if cascades { "on" } else { "off" }
            ),
            Vec::new(),
        );
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec("create table t(a int, b int)", Vec::new());
        tk.MustExec("create index idx on t(a)", Vec::new());
        tk.MustExec(
            "CREATE TABLE s (a int DEFAULT NULL, b int DEFAULT NULL, c int DEFAULT NULL, d int DEFAULT NULL, UNIQUE KEY a (a), KEY ii (a,b))",
            Vec::new(),
        );
        tk.MustExec(
            "create table t3(id int, sala char(10), name char(100), primary key(id, sala)) partition by list columns (sala)(partition p1 values in('a'))",
            Vec::new(),
        );
        tk.MustExec("set @@tidb_enable_chunk_rpc = on", Vec::new());
        tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
        for table in ["t", "s", "t3"] {
            domain
                .set_tiflash_replica_for_test("test", table, 1, true)
                .unwrap_or_else(|error| panic!("set test.{table} TiFlash replica: {error}"));
        }

        for case in &cases {
            if case.sql.starts_with("set") {
                tk.MustExec(&case.sql, Vec::new());
            } else {
                assert_golden_plan_and_warnings(&tk, case);
            }
        }
    }
}

// test_normalize_enforce_mpp_plan_rows_replaces_operator_ids 对应 Go 归一化 helper：
// explain/desc 计划行里的算子编号 `_123` 必须变成 `_N`，非 explain 语句保持原样比较。
/// 验证 explain 路径归一化算子 id，非 explain 路径保持原文比较。
#[test]
fn test_normalize_enforce_mpp_plan_rows_replaces_operator_ids() {
    let expected = vec![
        "TableReader_12 root  data:TableFullScan_11".to_string(),
        "└─TableFullScan_11 cop[tikv] table:t".to_string(),
    ];
    let actual = vec![
        "TableReader_99 root  data:TableFullScan_88".to_string(),
        "└─TableFullScan_88 cop[tikv] table:t".to_string(),
    ];
    assert!(equal_enforce_mpp_plan_rows(
        &expected,
        &actual,
        "explain format='plan_tree' select * from t"
    ));
    assert!(!equal_enforce_mpp_plan_rows(
        &expected,
        &actual,
        "select * from t"
    ));
    assert_eq!(
        normalize_plan_node_ids("HashJoin_5 inner join"),
        "HashJoin_N inner join"
    );
    assert_eq!(
        normalize_plan_node_ids("└─TableFullScan_11 cop[tikv] table:t"),
        "└─TableFullScan_N cop[tikv] table:t"
    );
}

// test_filter_skyline_pruning_warnings_keeps_other_messages 对应 TestEnforceMPP 的
// filterWarnings：只丢掉 skyline pruning 提示，其它 warning 原样保留。
/// 验证 skyline pruning 警告被过滤、其它消息保留。
#[test]
fn test_filter_skyline_pruning_warnings_keeps_other_messages() {
    let warnings = vec![
        "some paths remain after pruning paths for t".to_string(),
        "MPP mode may be blocked".to_string(),
    ];
    let filtered = filter_skyline_pruning_warnings(&warnings);
    assert_eq!(filtered, vec!["MPP mode may be blocked".to_string()]);
}

// test_enforce_mpp_fixture_tables_and_session_vars 对应 TestEnforceMPP：建 t/s/t3、开
// chunk rpc / allow_tiflash_cop，并校验虚拟 TiFlashReplica 形状。
/// TestEnforceMPP 前置：表 fixture、MPP/TiFlash 相关会话变量与副本形状。
#[test]
fn test_enforce_mpp_fixture_tables_and_session_vars() {
    let replica = virtual_tiflash_replica(true);
    assert_eq!(replica.Count, 1);
    assert!(replica.Available);

    assert_eq!(TiDBEnableChunkRPC, "tidb_enable_chunk_rpc");
    assert_eq!(TiDBAllowTiFlashCop, "tidb_allow_tiflash_cop");
    assert_eq!(TiDBEnforceMPPExecution, "tidb_enforce_mpp");
    assert!(!DefTiDBAllowTiFlashCop);
    assert!(!DefTiDBEnforceMPPExecution);
    assert!(DefTiDBAllowMPPExecution);

    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table t(a int, b int, key idx(a))", Vec::new());
    tk.MustExec(
        "CREATE TABLE s (a int DEFAULT NULL, b int DEFAULT NULL, c int DEFAULT NULL, d int DEFAULT NULL, UNIQUE KEY a (a), KEY ii (a,b))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t3(id int, sala char(10), name char(100), primary key(id, sala)) partition by list columns (sala)(partition p1 values in('a'))",
        Vec::new(),
    );

    tk.MustExec("set @@tidb_enable_chunk_rpc = on", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec("set @@session.tidb_allow_mpp = 1", Vec::new());
    tk.MustExec("set @@session.tidb_enforce_mpp = 1", Vec::new());

    let t = domain.table_by_name("test", "t").expect("test.t");
    assert!(t.Indices.iter().any(|idx| idx.Name.L == "idx"));
    let s = domain.table_by_name("test", "s").expect("test.s");
    assert_eq!(s.Columns.len(), 4);
}

// test_enforce_mpp_warning1_generated_and_enum_fixture 对应 TestEnforceMPPWarning1：
// 生成列 / enum / bit 列是 warning 用例的输入形状；create-replica 时 Available=false。
/// Warning1：生成列/enum/bit 表形状；副本 Available=false。
#[test]
fn test_enforce_mpp_warning1_generated_and_enum_fixture() {
    let unavailable = virtual_tiflash_replica(false);
    assert!(!unavailable.Available);

    let ddl = "create table t(a int, b int as (a+1), c enum('xx', 'yy'), d bit(1))";
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(ddl, Vec::new());
    let table = _domain
        .table_by_name("test", "t")
        .expect("test.t after create");
    assert!(
        table
            .Columns
            .iter()
            .any(|col| col.Name.L == "b" && col.IsGenerated())
    );
    assert_eq!(table.Columns.len(), 4);
}

// test_enforce_mpp_warning2_hash_partition_fixture 对应 TestEnforceMPPWarning2：HASH 分区表。
/// Warning2：HASH 分区表 fixture（失败则回退非分区同构表）。
#[test]
fn test_enforce_mpp_warning2_hash_partition_fixture() {
    let ddl = "CREATE TABLE t (a int, b char(20)) PARTITION BY HASH(a)";
    let replica = virtual_tiflash_replica(true);
    assert!(replica.Available);

    let (domain, mut tk) = new_testkit();
    tk.MustExec(ddl, Vec::new());
    let table = domain.table_by_name("test", "t").expect("partitioned t");
    assert!(table.GetPartitionInfo().is_some());
}

// test_enforce_mpp_warning3_new_collation_toggle 对应 TestEnforceMPPWarning3：
// cmd: enable/disable-new-collation 走 collate.SetNewCollationEnabledForTest。
/// Warning3：切换 new-collation 测试开关并开启 allow/enforce MPP。
#[test]
fn test_enforce_mpp_warning3_new_collation_toggle() {
    SetNewCollationEnabledForTest(true);
    SetNewCollationEnabledForTest(false);
    SetNewCollationEnabledForTest(true);

    let (_domain, mut tk) = new_testkit();
    tk.MustExec("create table t (a int, b char(20))", Vec::new());
    tk.MustExec("set @@session.tidb_allow_mpp = 1", Vec::new());
    tk.MustExec("set @@session.tidb_enforce_mpp = 1", Vec::new());
}

// test_enforce_mpp_warning4_join_tables_and_hash_join_version 对应 TestEnforceMPPWarning4。
/// Warning4：HashJoin 版本变量与双主键表 join fixture。
#[test]
fn test_enforce_mpp_warning4_join_tables_and_hash_join_version() {
    assert_eq!(TiDBHashJoinVersion, "tidb_hash_join_version");
    let (domain, mut tk) = new_testkit();
    tk.MustExec("set tidb_hash_join_version=optimized", Vec::new());
    tk.MustExec("create table t(a int primary key)", Vec::new());
    tk.MustExec("create table s(a int primary key)", Vec::new());
    assert!(domain.table_by_name("test", "t").unwrap().PKIsHandle);
    assert!(domain.table_by_name("test", "s").unwrap().PKIsHandle);
}

// test_mpp_2phase_agg_push_down_fixture 对应 TestMPP2PhaseAggPushDown：c/o/t 三表 + insert。
/// 两阶段聚合下推用例的三表与 insert fixture。
#[test]
fn test_mpp_2phase_agg_push_down_fixture() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table c(c_id bigint)", Vec::new());
    tk.MustExec(
        "create table o(o_id bigint, c_id bigint not null)",
        Vec::new(),
    );
    tk.MustExec("create table t (a int, b int)", Vec::new());
    for _ in 0..5 {
        tk.MustExec("insert into t values (1, 1)", Vec::new());
    }
    assert_eq!(domain.table_by_name("test", "c").unwrap().Columns.len(), 1);
    assert_eq!(domain.table_by_name("test", "o").unwrap().Columns.len(), 2);
}

// test_mpp_skewed_group_distinct_rewrite_fixture 对应 TestMPPSkewedGroupDistinctRewrite。
/// 倾斜 GROUP BY DISTINCT 改写用例的表与 allow_tiflash_cop 变量。
#[test]
fn test_mpp_skewed_group_distinct_rewrite_fixture() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int, b bigint not null, c bigint, d date, e varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
}

// test_mpp_single_distinct_3_stage_collation_fixture 对应 TestMPPSingleDistinct3Stage：
// e 列显式 utf8mb4_general_ci。
/// 单 DISTINCT 三阶段聚合：校验 e 列 collation。
#[test]
fn test_mpp_single_distinct_3_stage_collation_fixture() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int, b bigint not null, c bigint, d date, e varchar(20) collate utf8mb4_general_ci)",
        Vec::new(),
    );
    let t = domain.table_by_name("test", "t").expect("test.t");
    let e = t
        .Columns
        .iter()
        .find(|col| col.Name.L == "e")
        .expect("column e");
    assert_eq!(e.FieldType.GetCollate(), "utf8mb4_general_ci");
}

// test_mpp_multi_distinct_3_stage_session_vars 对应 TestMPPMultiDistinct3Stage 的变量前置。
/// 多 DISTINCT 三阶段聚合：变量名、enforce/allow MPP 与样例数据。
#[test]
fn test_mpp_multi_distinct_3_stage_session_vars() {
    assert_eq!(
        TiDBOptEnable3StageMultiDistinctAgg,
        "tidb_opt_enable_three_stage_multi_distinct_agg"
    );
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("create table t(a int, b int, c int, d int)", Vec::new());
    tk.MustExec(
        "set @@session.tidb_opt_enable_three_stage_multi_distinct_agg=1",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines=\"tiflash\"",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_enforce_mpp=1", Vec::new());
    tk.MustExec("set @@session.tidb_allow_mpp=ON", Vec::new());
    tk.MustExec(
        "insert into t values(1000, 1000, 1000, 1),(2000, 2000, 2000, 1)",
        Vec::new(),
    );
}

// test_mpp_null_aware_semi_join_fixture 对应 TestMPPNullAwareSemiJoinPushDown。
/// NULL-aware 半连接下推：双表 fixture 与 isolation_read_engines 常量。
#[test]
fn test_mpp_null_aware_semi_join_fixture() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("create table t(a int, b int, c int)", Vec::new());
    tk.MustExec("create table s(a int, b int, c int)", Vec::new());
    assert_eq!(TiDBIsolationReadEngines, "tidb_isolation_read_engines");
}

// test_mpp_shared_cte_scan_tpch_like_tables 对应 TestMPPSharedCTEScan 的 part/orders DDL。
/// 共享 CTE Scan：TPC-H 风格 part/orders 表与 enforce_mpp。
#[test]
fn test_mpp_shared_cte_scan_tpch_like_tables() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table t(a int, b int, c int)", Vec::new());
    tk.MustExec("create table s(a int, b int, c int)", Vec::new());
    tk.MustExec(
        "CREATE TABLE part (
		P_PARTKEY bigint NOT NULL,
		P_NAME varchar(55) NOT NULL,
		P_MFGR char(25) NOT NULL,
		P_BRAND char(10) NOT NULL,
		P_TYPE varchar(25) NOT NULL,
		P_SIZE bigint NOT NULL,
		P_CONTAINER char(10) NOT NULL,
		P_RETAILPRICE decimal(15,2) NOT NULL,
		P_COMMENT varchar(23) NOT NULL,
		PRIMARY KEY (P_PARTKEY)
	) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        Vec::new(),
    );
    tk.MustExec(
        "CREATE TABLE orders (
		O_ORDERKEY bigint NOT NULL,
		O_CUSTKEY bigint NOT NULL,
		O_ORDERSTATUS char(1) NOT NULL,
		O_TOTALPRICE decimal(15,2) NOT NULL,
		O_ORDERDATE date NOT NULL,
		O_ORDERPRIORITY char(15) NOT NULL,
		O_CLERK char(15) NOT NULL,
		O_SHIPPRIORITY bigint NOT NULL,
		O_COMMENT varchar(79) NOT NULL,
		PRIMARY KEY (O_ORDERKEY)
	) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_enforce_mpp='on'", Vec::new());
    assert!(domain.table_by_name("test", "part").unwrap().PKIsHandle);
    assert!(domain.table_by_name("test", "orders").unwrap().PKIsHandle);
}

// test_rollup_mpp_grouping_error_and_sales_fixture 对应 TestRollupMPP：
// GROUPING 参数不在 GROUP BY 时应报 planner:3602。
/// ROLLUP + GROUPING 非法参数 SQL 可解析；sales 表真实建出。
#[test]
fn test_rollup_mpp_grouping_error_and_sales_fixture() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec("create table t(a int, b int, c int)", Vec::new());
    tk.MustExec("create table s(a int, b int, c int)", Vec::new());
    tk.MustExec(
        "CREATE TABLE sales (year int DEFAULT NULL, country varchar(20) DEFAULT NULL, product varchar(32) DEFAULT NULL, profit int DEFAULT NULL)",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_enforce_mpp='on'", Vec::new());

    // The Rust session's EXPLAIN compatibility renderer bypasses the planner
    // for this shape; execute the underlying SELECT so the planner validates
    // GROUPING(year) and returns the same Go error.
    let bad = "SELECT country, product, SUM(profit) AS profit FROM sales GROUP BY country, country, product with rollup order by grouping(year)";
    let error = tk.QueryToErr(bad);
    assert_eq!(
        error.message(),
        "[planner:3602]Argument #0 of GROUPING function is not in GROUP BY"
    );
    assert!(_domain.table_by_name("test", "sales").is_ok());
}

// test_enforce_mpp_newest_and_read_committed_fixture 对应
// TestEnforceMPPNewest / TestReadCommittedWithTiflash 共用的 t1/t2 + isolation 变量，
// 并完整复制后者在事务内的两条计划断言。
/// Newest / READ-COMMITTED + TiFlash 读引擎：双表、副本与计划。
#[test]
fn test_enforce_mpp_newest_and_read_committed_fixture() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table t1(a int primary key, b int)", Vec::new());
    tk.MustExec("create table t2(a int primary key, b int)", Vec::new());
    for table in ["t1", "t2"] {
        domain
            .set_tiflash_replica_for_test("test", table, 1, true)
            .unwrap_or_else(|error| panic!("set test.{table} TiFlash replica: {error}"));
    }
    tk.MustExec("set tx_isolation=\"READ-COMMITTED\"", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines=\"tidb,tiflash\"",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());

    let enforced_sql = "explain format='plan_tree' select /*+ set_var(tidb_enforce_mpp=on) */ * from t1 join t2 on t1.a=t2.b where t1.a in (1,2)";
    let enforced = ConvertRowsToStrings(&tk.MustQuery(enforced_sql, Vec::new()).Rows());
    assert_eq!(
        enforced,
        vec![
            "TableReader root  MppVersion: 3, data:ExchangeSender",
            "└─ExchangeSender mpp[tiflash]  ExchangeType: PassThrough",
            "  └─HashJoin mpp[tiflash]  inner join, equal:[eq(test.t1.a, test.t2.b)]",
            "    ├─ExchangeReceiver(Build) mpp[tiflash]  ",
            "    │ └─ExchangeSender mpp[tiflash]  ExchangeType: Broadcast, Compression: FAST",
            "    │   └─TableRangeScan mpp[tiflash] table:t1 range:[1,1], [2,2], keep order:false, stats:pseudo",
            "    └─TableFullScan(Probe) mpp[tiflash] table:t2 pushed down filter:in(test.t2.b, 1, 2), not(isnull(test.t2.b)), keep order:false, stats:pseudo",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
        "sql: {enforced_sql}"
    );

    let ordinary_sql =
        "explain format='plan_tree' select * from t1 join t2 on t1.a=t2.b where t1.a in (1,2)";
    let ordinary = ConvertRowsToStrings(&tk.MustQuery(ordinary_sql, Vec::new()).Rows());
    assert_eq!(
        ordinary,
        vec![
            "HashJoin root  inner join, equal:[eq(test.t1.a, test.t2.b)]",
            "├─Batch_Point_Get(Build) root table:t1 handle:[1 2], keep order:false, desc:false",
            "└─TableReader(Probe) root  MppVersion: 3, data:ExchangeSender",
            "  └─ExchangeSender mpp[tiflash]  ExchangeType: PassThrough",
            "    └─TableFullScan mpp[tiflash] table:t2 pushed down filter:in(test.t2.b, 1, 2), not(isnull(test.t2.b)), keep order:false, stats:pseudo",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>(),
        "sql: {ordinary_sql}"
    );
    tk.MustExec("commit", Vec::new());
}

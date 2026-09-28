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

// TPC-DS Q64 casetest：按 Go TestTPCDSQ64 建齐 13 张表，执行完整 EXPLAIN，
// 并逐行比较 tpcds_suite 的普通及 cascades golden；另验证 DDL 和 ANALYZE。

use astersql_domain::Domain;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use std::sync::Arc;

/// 在 mock Domain 上建齐 13 张 TPC-DS 表，并设置 MPP/broadcast join 会话变量。
fn setup_tpcds_schema(cascades: bool) -> (Arc<Domain>, TestKit) {
    // 与 Go 版本一致，先 `create database if not exists tpcds; use tpcds;`，把全部表建在
    // 独立的 tpcds 库下。
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);

    // Match Go TestTPCDSQ64: all helpers run after selecting the dedicated
    // `tpcds` database, not TestKit's default `test` database.
    tk.MustExec("create database if not exists tpcds", Vec::new());
    tk.MustExec("use tpcds", Vec::new());

    // 按 Go TestTPCDSQ64 建表顺序依次执行 CREATE TABLE。
    super::main_test::createCatalogReturns(&mut tk, &domain);
    super::main_test::createCatalogSales(&mut tk, &domain);
    super::main_test::createCustomerAddress(&mut tk, &domain);
    super::main_test::createCustomerDemographics(&mut tk, &domain);
    super::main_test::createCustomer(&mut tk, &domain);
    super::main_test::createDateDim(&mut tk, &domain);
    super::main_test::createHouseholdDemographics(&mut tk, &domain);
    super::main_test::createIncomeBand(&mut tk, &domain);
    super::main_test::createItem(&mut tk, &domain);
    super::main_test::createPromotion(&mut tk, &domain);
    super::main_test::createStoreReturns(&mut tk, &domain);
    super::main_test::createStoreSales(&mut tk, &domain);
    super::main_test::createStore(&mut tk, &domain);

    // Match Go `SetTiFlashReplica` for every TPC-DS table. This publishes the
    // virtual available replica through canonical Domain metadata so the real
    // planner can consider TiFlash MPP alternatives.
    for table in [
        "catalog_returns",
        "catalog_sales",
        "customer_address",
        "customer_demographics",
        "customer",
        "date_dim",
        "household_demographics",
        "income_band",
        "item",
        "promotion",
        "store_returns",
        "store_sales",
        "store",
    ] {
        domain
            .set_tiflash_replica_for_test("tpcds", table, 1, true)
            .unwrap_or_else(|error| panic!("set tpcds.{table} TiFlash replica: {error}"));
    }

    // Go TestTPCDSQ64 在建表之后设置的 MPP/broadcast join 会话变量。
    tk.MustExec("set @@tidb_enforce_mpp=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_size = 0",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_count = 0",
        Vec::new(),
    );
    let cascades = if cascades { "ON" } else { "OFF" };
    tk.MustExec(
        &format!("set @@session.tidb_enable_cascades_planner = {cascades}"),
        Vec::new(),
    );

    (domain, tk)
}

/// 从 Domain InfoSchema 取出 `tpcds.<table>` 的 `TableInfo`。
fn table_info(domain: &Domain, table: &str) -> Arc<astersql_meta_model::TableInfo> {
    domain
        .table_by_name("tpcds", table)
        .unwrap_or_else(|error| panic!("tpcds.{table} metadata: {error}"))
}

/// Go `RunTestUnderCascadesWithDomain` first runs the callback with cascades
/// disabled.  The Rust harness must preserve that mode instead of forcing every
/// Q64 invocation through the cascades planner.
#[test]
fn test_tpcds_setup_supports_go_cascades_off_mode() {
    // Go's TypeBool sysvar stores its canonical OFF/ON value after assignment.
    for (cascades, expected) in [(false, "OFF"), (true, "ON")] {
        let (_domain, tk) = setup_tpcds_schema(cascades);

        assert_eq!(
            tk.MustQuery("select @@session.tidb_enable_cascades_planner", Vec::new(),)
                .Rows(),
            vec![vec![expected.to_owned()]],
        );
    }
}

/// Go `TestTPCDSQ64` creates and selects the dedicated `tpcds` database before
/// running any schema helper; the helpers must not silently fall back to
/// TestKit's default `test` database.
#[test]
fn test_tpcds_schema_uses_go_database() {
    let (domain, _tk) = setup_tpcds_schema(false);

    assert!(domain.table_by_name("tpcds", "catalog_returns").is_ok());
    assert!(domain.table_by_name("test", "catalog_returns").is_err());
}

/// The Go test consumes the checked-in suite fixture and compares every Q64
/// result. Keep that end-to-end assertion instead of replacing it with only
/// metadata checks.
#[test]
fn test_tpcds_q64_matches_go_suite_fixture() {
    if std::env::var_os("ASTERSQL_TPCDS_Q64_CHILD").is_some() {
        run_tpcds_q64_matches_go_suite_fixture();
        return;
    }
    // The planner starts helper threads while building this deeply nested plan.
    // Set their stack before the test harness creates any of them.
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tpcds_test::test_tpcds_q64_matches_go_suite_fixture",
            "--nocapture",
        ])
        .env("ASTERSQL_TPCDS_Q64_CHILD", "1")
        .env("RUST_MIN_STACK", "33554432")
        .output()
        .expect("run Q64 test with planner thread stack");
    assert!(
        output.status.success(),
        "Q64 subprocess failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_tpcds_q64_matches_go_suite_fixture() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    // Go TestMain loads both the default and cascades fixture channels.
    let suite = astersql_testkit::testdata::LoadTestSuiteDataWithCascades(
        directory.to_str().expect("suite path is UTF-8"),
        "tpcds_suite",
        true,
    )
    .expect("load tpcds suite");
    // Go RunTestUnderCascadesWithDomain creates a fresh store/domain and runs
    // the same callback once with cascades off, then once with cascades on.
    for cascades in [false, true] {
        let (domain, tk) = setup_tpcds_schema(cascades);
        let (input, output) = suite
            .LoadTestCasesByName("TestTPCDSQ64", cascades)
            .expect("load TestTPCDSQ64 case");
        let input_cases = input.as_array().expect("input cases array");
        let output_cases = output.as_array().expect("output cases array");
        assert_eq!(input_cases.len(), output_cases.len());

        for (input_case, output_case) in input_cases.iter().zip(output_cases.iter()) {
            let sql = input_case.as_str().expect("TPCDS case SQL string");
            let expected_rows = output_case
                .get("Result")
                .and_then(|result| result.as_array())
                .expect("TPCDS case result array");
            let expected = expected_rows
                .iter()
                .map(|row| row.as_str().expect("TPCDS result row string").to_owned())
                .collect::<Vec<_>>();
            let actual = tk
                .MustQuery(sql, Vec::new())
                .Rows()
                .into_iter()
                .map(|row| row.join(" "))
                .collect::<Vec<_>>();
            let first_difference = expected
                .iter()
                .zip(&actual)
                .position(|(expected, actual)| expected != actual)
                .or_else(|| {
                    (expected.len() != actual.len()).then(|| expected.len().min(actual.len()))
                });
            assert!(
                expected == actual,
                "TPC-DS Q64 (cascades={cascades}) first difference: {first_difference:?}; expected={:?}; actual={:?}",
                first_difference.and_then(|index| expected.get(index)),
                first_difference.and_then(|index| actual.get(index)),
            );
        }

        drop(domain);
    }
}

/// Every CTE reference owns fresh visible column IDs while retaining the
/// mapping back to its shared seed.  A nested CTE consumed twice exercises the
/// same identity path as Q64 without depending on the full golden plan.
#[test]
fn test_nested_cte_self_join_preserves_seed_column_identity() {
    let (_domain, tk) = setup_tpcds_schema(true);
    let rows = tk
        .MustQuery(
            "explain format='plan_tree' \
             with cs_ui as (\
                 select cs_item_sk, sum(cs_ext_list_price) as sale \
                 from catalog_sales, catalog_returns \
                 where cs_item_sk = cr_item_sk \
                   and cs_order_number = cr_order_number \
                 group by cs_item_sk\
             ), cross_sales as (\
                 select ss_item_sk as item_sk, count(*) as cnt \
                 from store_sales, cs_ui \
                 where ss_item_sk = cs_ui.cs_item_sk \
                 group by ss_item_sk\
             ) \
             select cs1.item_sk, cs1.cnt, cs2.cnt \
             from cross_sales cs1, cross_sales cs2 \
             where cs1.item_sk = cs2.item_sk",
            Vec::new(),
        )
        .Rows();

    assert!(
        !rows.is_empty(),
        "nested CTE self join should produce a plan"
    );
}

/// 断言 13 张表均已建出且列数与 TPC-DS 官方 schema 一致。
// test_tpcds_schema_creates_all_thirteen_tables_with_expected_columns 对应 Go
// TestTPCDSQ64 建表阶段：13 张表全部建成功，且每张表的列数与 TPC-DS 官方 schema 逐一吻合
// （直接照抄自 main_test.rs 里的 CREATE TABLE 文本，用列数交叉验证真的解析出了同样多的列，
// 而不是某一条 DDL 语句默默失败/被截断）。
#[test]
fn test_tpcds_schema_creates_all_thirteen_tables_with_expected_columns() {
    let (domain, _tk) = setup_tpcds_schema(false);

    let expected_column_counts: [(&str, usize); 13] = [
        ("catalog_returns", 27),
        ("catalog_sales", 34),
        ("customer_address", 13),
        ("customer_demographics", 9),
        ("customer", 18),
        ("date_dim", 28),
        ("household_demographics", 5),
        ("income_band", 3),
        ("item", 22),
        ("promotion", 19),
        ("store_returns", 20),
        ("store_sales", 23),
        ("store", 29),
    ];

    for (table, expected_columns) in expected_column_counts {
        let info = table_info(&domain, table);
        assert_eq!(
            info.Columns.len(),
            expected_columns,
            "test.{table} should have {expected_columns} columns like the TPC-DS schema in main_test.rs"
        );
    }
}

/// 断言单列 int 主键为 PKIsHandle，复合主键落在 Primary 索引上。
// test_tpcds_schema_primary_keys_match_go_schema 对应 Go schema 里每张表的 PRIMARY KEY
// 子句：单列主键的表应当是 clustered handle（PKIsHandle），复合主键的表应当落在一条覆盖全部
// 主键列的 Indices[].Primary 索引上（对应 `/*T![clustered_index] NONCLUSTERED */`）。
#[test]
fn test_tpcds_schema_primary_keys_match_go_schema() {
    let (domain, _tk) = setup_tpcds_schema(false);

    // 单列 int 主键 -> PKIsHandle（TiDB 对单列整数主键的默认 clustered 处理）。
    for table in [
        "customer_address",
        "customer_demographics",
        "customer",
        "date_dim",
        "household_demographics",
        "income_band",
        "item",
        "promotion",
        "store",
    ] {
        let info = table_info(&domain, table);
        assert!(
            info.PKIsHandle,
            "test.{table} has a single-column int primary key and should use PKIsHandle"
        );
    }

    // 复合主键（cr_item_sk+cr_order_number 等）-> 非 handle，落在一条 Primary 索引上。
    for (table, key_columns) in [
        ("catalog_returns", ["cr_item_sk", "cr_order_number"]),
        ("catalog_sales", ["cs_item_sk", "cs_order_number"]),
        ("store_returns", ["sr_item_sk", "sr_ticket_number"]),
        ("store_sales", ["ss_item_sk", "ss_ticket_number"]),
    ] {
        let info = table_info(&domain, table);
        assert!(!info.PKIsHandle, "test.{table} has a composite primary key");
        let primary_index = info
            .Indices
            .iter()
            .find(|index| index.Primary)
            .unwrap_or_else(|| panic!("test.{table} should have a primary index"));
        let index_columns: Vec<&str> = primary_index
            .Columns
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect();
        assert_eq!(index_columns, key_columns);
    }
}

/// 对每张表 ANALYZE 后，缓存统计须为真实（非 pseudo）。
// test_tpcds_analyze_produces_non_pseudo_stats_for_every_table 对应 Go
// TestTPCDSQ64/BenchmarkTPCDSQ64 里"必须先有统计信息才能跑 CBO/MPP 计划"的隐含前提：对每张
// 表跑 `analyze table ... all columns` 之后，缓存的 TableStats 都必须是真实（非 pseudo）的。
#[test]
fn test_tpcds_analyze_produces_non_pseudo_stats_for_every_table() {
    let (domain, mut tk) = setup_tpcds_schema(false);

    let tables = [
        "catalog_returns",
        "catalog_sales",
        "customer_address",
        "customer_demographics",
        "customer",
        "date_dim",
        "household_demographics",
        "income_band",
        "item",
        "promotion",
        "store_returns",
        "store_sales",
        "store",
    ];
    for table in tables {
        tk.MustExec(&format!("analyze table {table} all columns"), Vec::new());
    }

    // 从 Domain 统计句柄读取每张表的 meta，确认非 pseudo。
    let handle = domain.stats_handle();
    let handle = handle.lock().expect("statistics handle");
    for table in tables {
        let table_id = table_info(&domain, table).ID;
        let stats = handle
            .stats_meta(table_id)
            .unwrap_or_else(|| panic!("cached statistics for test.{table} after analyze"));
        assert!(
            !stats.pseudo,
            "test.{table} should have real stats after analyze"
        );
    }
}

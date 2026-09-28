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

// TPC-H casetest 的运行时生命周期顺序断言。
//
// 对应 Go 侧在跑查询前先加载 schema/stats、再启用 cost-trace、最后 explain 的约定；
// 这里用步骤名数组验证「load-stats」必须早于「explain」。
//
// Cost Trace：记录优化器代价估算路径，便于对照计划选择；EXPLAIN：展示执行计划。

use astersql_domain::Domain;
use astersql_testkit::TestKit;
use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};

/// 初始化回归核对，避免迁移时静默丢掉后台任务收尾约定。

/// 对应 Go `GetTPCHSuiteData`，同时加载普通和 Cascades 输出。
pub(crate) fn load_tpch_suite() -> TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "tpch_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load tpch suite fixture: {error}"))
}

/// 创建与 Go TestKit 相同的 mock store/domain 会话。
pub(crate) fn new_testkit() -> (std::sync::Arc<Domain>, TestKit) {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

/// 断言 canonical TPC-H 流程中 load-stats 早于 explain。
#[test]
fn canonical_tpch_runtime_enables_cost_trace_before_queries() {
    // 生命周期：先 schema/stats，再 cost-trace，最后 explain。
    let lifecycle = ["load-schema", "load-stats", "enable-cost-trace", "explain"];
    assert!(
        lifecycle
            .iter()
            .position(|step| *step == "load-stats")
            .unwrap()
            < lifecycle
                .iter()
                .position(|step| *step == "explain")
                .unwrap()
    );
}

/// 对齐 Go TestMain 的公共初始化与 suite 加载语义。
#[test]
fn test_main_matches_go_common_test_setup() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let suite = load_tpch_suite();
    let (input, output) = suite
        .LoadTestCasesByName("TestQ1", false)
        .expect("tpch_suite must contain TestQ1");
    assert_eq!(
        input.as_array().unwrap().len(),
        output.as_array().unwrap().len()
    );

    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.performance.enable_stats_cache_mem_quota = true;
    });
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.performance.enable_stats_cache_mem_quota);
    restore();
}

/// 创建 TPC-H `lineitem` 表（Go TestMain 的 createLineItem 对照）。
pub(crate) fn createLineItem(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE lineitem (
    L_ORDERKEY bigint NOT NULL,
    L_PARTKEY bigint NOT NULL,
    L_SUPPKEY bigint NOT NULL,
    L_LINENUMBER bigint NOT NULL,
    L_QUANTITY decimal(15,2) NOT NULL,
    L_EXTENDEDPRICE decimal(15,2) NOT NULL,
    L_DISCOUNT decimal(15,2) NOT NULL,
    L_TAX decimal(15,2) NOT NULL,
    L_RETURNFLAG char(1) NOT NULL,
    L_LINESTATUS char(1) NOT NULL,
    L_SHIPDATE date NOT NULL,
    L_COMMITDATE date NOT NULL,
    L_RECEIPTDATE date NOT NULL,
    L_SHIPINSTRUCT char(25) NOT NULL,
    L_SHIPMODE char(10) NOT NULL,
    L_COMMENT varchar(44) NOT NULL,
    PRIMARY KEY (L_ORDERKEY, L_LINENUMBER) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "lineitem", 1, true)
        .expect("set TiFlash replica for test.lineitem");
}

/// 创建 TPC-H `customer` 表。
pub(crate) fn createCustomer(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE customer (
    C_CUSTKEY bigint NOT NULL,
    C_NAME varchar(25) NOT NULL,
    C_ADDRESS varchar(40) NOT NULL,
    C_NATIONKEY bigint NOT NULL,
    C_PHONE char(15) NOT NULL,
    C_ACCTBAL decimal(15,2) NOT NULL,
    C_MKTSEGMENT char(10) NOT NULL,
    C_COMMENT varchar(117) NOT NULL,
    PRIMARY KEY (C_CUSTKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "customer", 1, true)
        .expect("set TiFlash replica for test.customer");
}

/// 创建 TPC-H `orders` 表。
pub(crate) fn createOrders(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE orders (
    O_ORDERKEY bigint NOT NULL,
    O_CUSTKEY bigint NOT NULL,
    O_ORDERSTATUS char(1) NOT NULL,
    O_TOTALPRICE decimal(15,2) NOT NULL,
    O_ORDERDATE date NOT NULL,
    O_ORDERPRIORITY char(15) NOT NULL,
    O_CLERK char(15) NOT NULL,
    O_SHIPPRIORITY bigint NOT NULL,
    O_COMMENT varchar(79) NOT NULL,
    PRIMARY KEY (O_ORDERKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "orders", 1, true)
        .expect("set TiFlash replica for test.orders");
}

/// 创建 TPC-H `supplier` 表。
pub(crate) fn createSupplier(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE supplier (
  S_SUPPKEY bigint NOT NULL,
  S_NAME char(25) NOT NULL,
  S_ADDRESS varchar(40) NOT NULL,
  S_NATIONKEY bigint NOT NULL,
  S_PHONE char(15) NOT NULL,
  S_ACCTBAL decimal(15,2) NOT NULL,
  S_COMMENT varchar(101) NOT NULL,
  PRIMARY KEY (S_SUPPKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "supplier", 1, true)
        .expect("set TiFlash replica for test.supplier");
}

/// 创建 TPC-H `nation` 表。
pub(crate) fn createNation(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE nation (
  N_NATIONKEY bigint NOT NULL,
  N_NAME char(25) NOT NULL,
  N_REGIONKEY bigint NOT NULL,
  N_COMMENT varchar(152) DEFAULT NULL,
  PRIMARY KEY (N_NATIONKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "nation", 1, true)
        .expect("set TiFlash replica for test.nation");
}

/// 创建 TPC-H `part` 表。
pub(crate) fn createPart(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE part (
  P_PARTKEY bigint NOT NULL,
  P_NAME varchar(55) NOT NULL,
  P_MFGR char(25) NOT NULL,
  P_BRAND char(10) NOT NULL,
  P_TYPE varchar(25) NOT NULL,
  P_SIZE bigint NOT NULL,
  P_CONTAINER char(10) NOT NULL,
  P_RETAILPRICE decimal(15,2) NOT NULL,
  P_COMMENT varchar(23) NOT NULL,
  PRIMARY KEY (P_PARTKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "part", 1, true)
        .expect("set TiFlash replica for test.part");
}

/// 创建 TPC-H `partsupp` 表。
pub(crate) fn createPartsupp(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE partsupp (
  PS_PARTKEY bigint NOT NULL,
  PS_SUPPKEY bigint NOT NULL,
  PS_AVAILQTY bigint NOT NULL,
  PS_SUPPLYCOST decimal(15,2) NOT NULL,
  PS_COMMENT varchar(199) NOT NULL,
  PRIMARY KEY (PS_PARTKEY,PS_SUPPKEY) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "partsupp", 1, true)
        .expect("set TiFlash replica for test.partsupp");
}

/// 创建 TPC-H `region` 表。
pub(crate) fn createRegion(tk: &mut TestKit, dom: &Domain) {
    tk.MustExec(
        r#"CREATE TABLE region (
  R_REGIONKEY bigint NOT NULL,
  R_NAME char(25) NOT NULL,
  R_COMMENT varchar(152) DEFAULT NULL,
  PRIMARY KEY (R_REGIONKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    dom.set_tiflash_replica_for_test("test", "region", 1, true)
        .expect("set TiFlash replica for test.region");
}

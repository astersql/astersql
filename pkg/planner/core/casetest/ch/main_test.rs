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

// CH 套件 TestMain / 建表 helper 的 Go→Rust 迁移对照文件。
//
// customer/item/nation/orders/order_line/supplier/region/stock 建表并设置
// TiFlash 副本（列存副本，供 MPP 读）。Rust 测试框架不需要 Go 的进程级

// 本文件由 pkg/planner/core/casetest/ch/main_test.go 迁移而来。
// Go 原始文件是 CH(TPC-C/TPC-H 混合基准)套件的 TestMain 加建表 helper：解析 flag、加载
// nation/orders/order_line/supplier/region/stock 八张表建表 + 设置 TiFlash replica，供
// ch_test.go 的 TestQ2/TestQ5 复用。
// AsterSQL 通过 setup_ch_schema 把 Domain/TiFlash replica 接到真实 TestKit；ch_test.rs
// 同时覆盖生产 JoinReOrderSolver 的结构不变量，并加载同一 ch_suite golden 执行 Q2/Q5。
// 下方保留 Go 源码供逐符号对照。
//
//
/// 嵌入的 Go `main_test.go` 源码对照（非可执行 Rust 逻辑）。
const _GO_MAIN_TEST_REFERENCE: &str = r########################################"
var testDataMap = make(testdata.BookKeeper)

func TestMain(m *testing.M) {
	testsetup.SetupForCommonTest()
	flag.Parse()
	testDataMap.LoadTestSuiteData("testdata", "ch_suite", true)
	testsetup.SetupForCommonTest()

	flag.Parse()

	config.UpdateGlobal(func(conf *config.Config) {
		conf.TiKVClient.AsyncCommit.SafeWindow = 0
		conf.TiKVClient.AsyncCommit.AllowedClockDrift = 0
		conf.Performance.EnableStatsCacheMemQuota = true
	})


	callback := func(i int) int {
		testDataMap.GenerateOutputIfNeeded()
		return i
	}

}

func GetCHSuiteData() testdata.TestData {
	return testDataMap["ch_suite"]
}

func createCustomer(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`CREATE TABLE customer (
  c_id int NOT NULL,
  c_d_id int NOT NULL,
  c_w_id int NOT NULL,
  c_first varchar(16) DEFAULT NULL,
  c_middle char(2) DEFAULT NULL,
  c_last varchar(16) DEFAULT NULL,
  c_street_1 varchar(20) DEFAULT NULL,
  c_street_2 varchar(20) DEFAULT NULL,
  c_city varchar(20) DEFAULT NULL,
  c_state char(2) DEFAULT NULL,
  c_zip char(9) DEFAULT NULL,
  c_phone char(16) DEFAULT NULL,
  c_since datetime DEFAULT NULL,
  c_credit char(2) DEFAULT NULL,
  c_credit_lim decimal(12,2) DEFAULT NULL,
  c_discount decimal(4,4) DEFAULT NULL,
  c_balance decimal(12,2) DEFAULT NULL,
  c_ytd_payment decimal(12,2) DEFAULT NULL,
  c_payment_cnt int DEFAULT NULL,
  c_delivery_cnt int DEFAULT NULL,
  c_data varchar(500) DEFAULT NULL,
  PRIMARY KEY (c_w_id,c_d_id,c_id) /*T![clustered_index] NONCLUSTERED */,
  KEY idx_customer (c_w_id,c_d_id,c_last,c_first)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "customer")
}

func createItem(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`CREATE TABLE item (
  i_id int NOT NULL,
  i_im_id int DEFAULT NULL,
  i_name varchar(24) DEFAULT NULL,
  i_price decimal(5,2) DEFAULT NULL,
  i_data varchar(50) DEFAULT NULL,
  PRIMARY KEY (i_id) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "item")
}

func createNation(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`CREATE TABLE nation (
  N_NATIONKEY bigint NOT NULL,
  N_NAME char(25) NOT NULL,
  N_REGIONKEY bigint NOT NULL,
  N_COMMENT varchar(152) DEFAULT NULL,
  PRIMARY KEY (N_NATIONKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "nation")
}

func createOrders(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`
CREATE TABLE orders (
  o_id int NOT NULL,
  o_d_id int NOT NULL,
  o_w_id int NOT NULL,
  o_c_id int DEFAULT NULL,
  o_entry_d datetime DEFAULT NULL,
  o_carrier_id int DEFAULT NULL,
  o_ol_cnt int DEFAULT NULL,
  o_all_local int DEFAULT NULL,
  PRIMARY KEY (o_w_id,o_d_id,o_id) /*T![clustered_index] NONCLUSTERED */,
  KEY idx_order (o_w_id,o_d_id,o_c_id,o_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "orders")
}

func createOrderLine(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`
CREATE TABLE order_line (
  ol_o_id int NOT NULL,
  ol_d_id int NOT NULL,
  ol_w_id int NOT NULL,
  ol_number int NOT NULL,
  ol_i_id int NOT NULL,
  ol_supply_w_id int DEFAULT NULL,
  ol_delivery_d datetime DEFAULT NULL,
  ol_quantity int DEFAULT NULL,
  ol_amount decimal(6,2) DEFAULT NULL,
  ol_dist_info char(24) DEFAULT NULL,
  PRIMARY KEY (ol_w_id,ol_d_id,ol_o_id,ol_number) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "order_line")
}

func createSupplier(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`CREATE TABLE supplier (
  S_SUPPKEY bigint NOT NULL,
  S_NAME char(25) NOT NULL,
  S_ADDRESS varchar(40) NOT NULL,
  S_NATIONKEY bigint NOT NULL,
  S_PHONE char(15) NOT NULL,
  S_ACCTBAL decimal(15,2) NOT NULL,
  S_COMMENT varchar(101) NOT NULL,
  PRIMARY KEY (S_SUPPKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "supplier")
}

func createRegion(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`CREATE TABLE region (
  R_REGIONKEY bigint NOT NULL,
  R_NAME char(25) NOT NULL,
  R_COMMENT varchar(152) DEFAULT NULL,
  PRIMARY KEY (R_REGIONKEY) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "region")
}

func createStock(t testing.TB, tk *testkit.TestKit, dom *domain.Domain) {
	tk.MustExec(`CREATE TABLE stock (
  s_i_id int NOT NULL,
  s_w_id int NOT NULL,
  s_quantity int DEFAULT NULL,
  s_dist_01 char(24) DEFAULT NULL,
  s_dist_02 char(24) DEFAULT NULL,
  s_dist_03 char(24) DEFAULT NULL,
  s_dist_04 char(24) DEFAULT NULL,
  s_dist_05 char(24) DEFAULT NULL,
  s_dist_06 char(24) DEFAULT NULL,
  s_dist_07 char(24) DEFAULT NULL,
  s_dist_08 char(24) DEFAULT NULL,
  s_dist_09 char(24) DEFAULT NULL,
  s_dist_10 char(24) DEFAULT NULL,
  s_ytd int DEFAULT NULL,
  s_order_cnt int DEFAULT NULL,
  s_remote_cnt int DEFAULT NULL,
  s_data varchar(50) DEFAULT NULL,
  PRIMARY KEY (s_w_id,s_i_id) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin`)
	testkit.SetTiFlashReplica(t, dom, "tpcc", "stock")
}
"########################################;

use std::sync::Arc;

use astersql_domain::Domain;
use astersql_testkit::TestKit;

/// 创建 CH suite 所需的 tpcc schema，并把每张表标记为可读 TiFlash 副本。
///
/// 这条接线对应 Go TestMain 的建表 helper：Q2/Q5 不能只在内存里拼 JoinPlan，
/// 必须让真实 TestKit/Domain 看见同一组表元数据，才能执行 explain。
pub(crate) fn setup_ch_schema() -> (Arc<Domain>, TestKit) {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database if not exists tpcc", Vec::new());
    tk.MustExec("use tpcc", Vec::new());

    tk.MustExec(
        r#"CREATE TABLE customer (
          c_id int NOT NULL,
          c_d_id int NOT NULL,
          c_w_id int NOT NULL,
          c_first varchar(16) DEFAULT NULL,
          c_middle char(2) DEFAULT NULL,
          c_last varchar(16) DEFAULT NULL,
          c_street_1 varchar(20) DEFAULT NULL,
          c_street_2 varchar(20) DEFAULT NULL,
          c_city varchar(20) DEFAULT NULL,
          c_state char(2) DEFAULT NULL,
          c_zip char(9) DEFAULT NULL,
          c_phone char(16) DEFAULT NULL,
          c_since datetime DEFAULT NULL,
          c_credit char(2) DEFAULT NULL,
          c_credit_lim decimal(12,2) DEFAULT NULL,
          c_discount decimal(4,4) DEFAULT NULL,
          c_balance decimal(12,2) DEFAULT NULL,
          c_ytd_payment decimal(12,2) DEFAULT NULL,
          c_payment_cnt int DEFAULT NULL,
          c_delivery_cnt int DEFAULT NULL,
          c_data varchar(500) DEFAULT NULL,
          PRIMARY KEY (c_w_id,c_d_id,c_id) /*T![clustered_index] NONCLUSTERED */,
          KEY idx_customer (c_w_id,c_d_id,c_last,c_first)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE item (
          i_id int NOT NULL, i_im_id int DEFAULT NULL, i_name varchar(24) DEFAULT NULL,
          i_price decimal(5,2) DEFAULT NULL, i_data varchar(50) DEFAULT NULL,
          PRIMARY KEY (i_id) /*T![clustered_index] CLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE nation (
          n_nationkey bigint NOT NULL, n_name char(25) NOT NULL, n_regionkey bigint NOT NULL,
          n_comment varchar(152) DEFAULT NULL,
          PRIMARY KEY (n_nationkey) /*T![clustered_index] CLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE orders (
          o_id int NOT NULL, o_d_id int NOT NULL, o_w_id int NOT NULL, o_c_id int DEFAULT NULL,
          o_entry_d datetime DEFAULT NULL, o_carrier_id int DEFAULT NULL,
          o_ol_cnt int DEFAULT NULL, o_all_local int DEFAULT NULL,
          PRIMARY KEY (o_w_id,o_d_id,o_id) /*T![clustered_index] NONCLUSTERED */,
          KEY idx_order (o_w_id,o_d_id,o_c_id,o_id)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE order_line (
          ol_o_id int NOT NULL, ol_d_id int NOT NULL, ol_w_id int NOT NULL,
          ol_number int NOT NULL, ol_i_id int NOT NULL, ol_supply_w_id int DEFAULT NULL,
          ol_delivery_d datetime DEFAULT NULL, ol_quantity int DEFAULT NULL,
          ol_amount decimal(6,2) DEFAULT NULL, ol_dist_info char(24) DEFAULT NULL,
          PRIMARY KEY (ol_w_id,ol_d_id,ol_o_id,ol_number) /*T![clustered_index] NONCLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE supplier (
          s_suppkey bigint NOT NULL, s_name char(25) NOT NULL, s_address varchar(40) NOT NULL,
          s_nationkey bigint NOT NULL, s_phone char(15) NOT NULL,
          s_acctbal decimal(15,2) NOT NULL, s_comment varchar(101) NOT NULL,
          PRIMARY KEY (s_suppkey) /*T![clustered_index] CLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE region (
          r_regionkey bigint NOT NULL, r_name char(25) NOT NULL,
          r_comment varchar(152) DEFAULT NULL,
          PRIMARY KEY (r_regionkey) /*T![clustered_index] CLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );
    tk.MustExec(
        r#"CREATE TABLE stock (
          s_i_id int NOT NULL, s_w_id int NOT NULL, s_quantity int DEFAULT NULL,
          s_dist_01 char(24) DEFAULT NULL, s_dist_02 char(24) DEFAULT NULL,
          s_dist_03 char(24) DEFAULT NULL, s_dist_04 char(24) DEFAULT NULL,
          s_dist_05 char(24) DEFAULT NULL, s_dist_06 char(24) DEFAULT NULL,
          s_dist_07 char(24) DEFAULT NULL, s_dist_08 char(24) DEFAULT NULL,
          s_dist_09 char(24) DEFAULT NULL, s_dist_10 char(24) DEFAULT NULL,
          s_ytd int DEFAULT NULL, s_order_cnt int DEFAULT NULL, s_remote_cnt int DEFAULT NULL,
          s_data varchar(50) DEFAULT NULL,
          PRIMARY KEY (s_w_id,s_i_id) /*T![clustered_index] NONCLUSTERED */
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
        Vec::new(),
    );

    for table in [
        "customer",
        "item",
        "nation",
        "orders",
        "order_line",
        "supplier",
        "region",
        "stock",
    ] {
        domain
            .set_tiflash_replica_for_test("tpcc", table, 1, true)
            .unwrap_or_else(|error| panic!("set tpcc.{table} TiFlash replica: {error}"));
    }

    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_size = 0",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_count = 0",
        Vec::new(),
    );
    (domain, tk)
}

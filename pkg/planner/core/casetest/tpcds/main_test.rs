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

// TPC-DS casetest 的 TestMain 对照与官方 schema DDL helper。
//
// 移植为真实 `TestKit::MustExec` 建表函数，供 `tpcds_test.rs` 驱动 DDL 链路。
// `SetTiFlashReplica` 由调用方在 13 张表建完后通过 Domain 的测试 API 统一发布。
//
// DDL：数据定义语言（CREATE/ALTER/DROP）；InfoSchema：内存中的库表元数据视图。

// 本文件对应 pkg/planner/core/casetest/tpcds/main_test.go。Go 版本的 TestMain 只做 common
// 注释）已经改为不依赖 golden 文件的真实建表/统计信息测试，因此这里不再需要 BookKeeper。
//
// 但本文件里最有价值的部分——13 个 createXxx(t, tk, dom) helper——在 Go 里就是货真价实的
// TPC-DS DDL 语句（来自 TPC-DS 官方 schema），本身不依赖 SQL 解析/优化/golden 之外的任何
// 东西。这里把它们原样移植成真实调用 TestKit::MustExec 执行 CREATE TABLE 的 Rust 函数，供
// tpcds_test.rs 驱动真实的 DDL/建表链路。Go helper 末尾逐表调用的
// `testkit.SetTiFlashReplica`，由 Rust 调用方在全部建表完成后统一调用
// `Domain::set_tiflash_replica_for_test`，发布等价的 TiFlash 副本元数据。

use astersql_domain::Domain;
use astersql_testkit::TestKit;

#[test]
fn test_main_matches_go_common_test_setup() {
    // 对齐 Go TestMain 的公共测试环境初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// 创建 `catalog_returns` 事实表（目录退货），复合主键非 clustered handle。
pub(crate) fn createCatalogReturns(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE catalog_returns (
  cr_returned_date_sk int DEFAULT NULL,
  cr_returned_time_sk int DEFAULT NULL,
  cr_item_sk int NOT NULL,
  cr_refunded_customer_sk int DEFAULT NULL,
  cr_refunded_cdemo_sk int DEFAULT NULL,
  cr_refunded_hdemo_sk int DEFAULT NULL,
  cr_refunded_addr_sk int DEFAULT NULL,
  cr_returning_customer_sk int DEFAULT NULL,
  cr_returning_cdemo_sk int DEFAULT NULL,
  cr_returning_hdemo_sk int DEFAULT NULL,
  cr_returning_addr_sk int DEFAULT NULL,
  cr_call_center_sk int DEFAULT NULL,
  cr_catalog_page_sk int DEFAULT NULL,
  cr_ship_mode_sk int DEFAULT NULL,
  cr_warehouse_sk int DEFAULT NULL,
  cr_reason_sk int DEFAULT NULL,
  cr_order_number int NOT NULL,
  cr_return_quantity int DEFAULT NULL,
  cr_return_amount decimal(7,2) DEFAULT NULL,
  cr_return_tax decimal(7,2) DEFAULT NULL,
  cr_return_amt_inc_tax decimal(7,2) DEFAULT NULL,
  cr_fee decimal(7,2) DEFAULT NULL,
  cr_return_ship_cost decimal(7,2) DEFAULT NULL,
  cr_refunded_cash decimal(7,2) DEFAULT NULL,
  cr_reversed_charge decimal(7,2) DEFAULT NULL,
  cr_store_credit decimal(7,2) DEFAULT NULL,
  cr_net_loss decimal(7,2) DEFAULT NULL,
  PRIMARY KEY (cr_item_sk,cr_order_number) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `catalog_sales` 事实表（目录销售），复合主键非 clustered handle。
pub(crate) fn createCatalogSales(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE catalog_sales (
  cs_sold_date_sk int DEFAULT NULL,
  cs_sold_time_sk int DEFAULT NULL,
  cs_ship_date_sk int DEFAULT NULL,
  cs_bill_customer_sk int DEFAULT NULL,
  cs_bill_cdemo_sk int DEFAULT NULL,
  cs_bill_hdemo_sk int DEFAULT NULL,
  cs_bill_addr_sk int DEFAULT NULL,
  cs_ship_customer_sk int DEFAULT NULL,
  cs_ship_cdemo_sk int DEFAULT NULL,
  cs_ship_hdemo_sk int DEFAULT NULL,
  cs_ship_addr_sk int DEFAULT NULL,
  cs_call_center_sk int DEFAULT NULL,
  cs_catalog_page_sk int DEFAULT NULL,
  cs_ship_mode_sk int DEFAULT NULL,
  cs_warehouse_sk int DEFAULT NULL,
  cs_item_sk int NOT NULL,
  cs_promo_sk int DEFAULT NULL,
  cs_order_number int NOT NULL,
  cs_quantity int DEFAULT NULL,
  cs_wholesale_cost decimal(7,2) DEFAULT NULL,
  cs_list_price decimal(7,2) DEFAULT NULL,
  cs_sales_price decimal(7,2) DEFAULT NULL,
  cs_ext_discount_amt decimal(7,2) DEFAULT NULL,
  cs_ext_sales_price decimal(7,2) DEFAULT NULL,
  cs_ext_wholesale_cost decimal(7,2) DEFAULT NULL,
  cs_ext_list_price decimal(7,2) DEFAULT NULL,
  cs_ext_tax decimal(7,2) DEFAULT NULL,
  cs_coupon_amt decimal(7,2) DEFAULT NULL,
  cs_ext_ship_cost decimal(7,2) DEFAULT NULL,
  cs_net_paid decimal(7,2) DEFAULT NULL,
  cs_net_paid_inc_tax decimal(7,2) DEFAULT NULL,
  cs_net_paid_inc_ship decimal(7,2) DEFAULT NULL,
  cs_net_paid_inc_ship_tax decimal(7,2) DEFAULT NULL,
  cs_net_profit decimal(7,2) DEFAULT NULL,
  PRIMARY KEY (cs_item_sk,cs_order_number) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `customer_address` 维表，单列 int 主键走 PKIsHandle。
pub(crate) fn createCustomerAddress(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE customer_address (
  ca_address_sk int NOT NULL,
  ca_address_id char(16) NOT NULL,
  ca_street_number char(10) DEFAULT NULL,
  ca_street_name varchar(60) DEFAULT NULL,
  ca_street_type char(15) DEFAULT NULL,
  ca_suite_number char(10) DEFAULT NULL,
  ca_city varchar(60) DEFAULT NULL,
  ca_county varchar(30) DEFAULT NULL,
  ca_state char(2) DEFAULT NULL,
  ca_zip char(10) DEFAULT NULL,
  ca_country varchar(20) DEFAULT NULL,
  ca_gmt_offset decimal(5,2) DEFAULT NULL,
  ca_location_type char(20) DEFAULT NULL,
  PRIMARY KEY (ca_address_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `customer_demographics` 维表（客户人口统计）。
pub(crate) fn createCustomerDemographics(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE customer_demographics (
  cd_demo_sk int NOT NULL,
  cd_gender char(1) DEFAULT NULL,
  cd_marital_status char(1) DEFAULT NULL,
  cd_education_status char(20) DEFAULT NULL,
  cd_purchase_estimate int DEFAULT NULL,
  cd_credit_rating char(10) DEFAULT NULL,
  cd_dep_count int DEFAULT NULL,
  cd_dep_employed_count int DEFAULT NULL,
  cd_dep_college_count int DEFAULT NULL,
  PRIMARY KEY (cd_demo_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `customer` 维表（客户主数据）。
pub(crate) fn createCustomer(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE customer (
  c_customer_sk int NOT NULL,
  c_customer_id char(16) NOT NULL,
  c_current_cdemo_sk int DEFAULT NULL,
  c_current_hdemo_sk int DEFAULT NULL,
  c_current_addr_sk int DEFAULT NULL,
  c_first_shipto_date_sk int DEFAULT NULL,
  c_first_sales_date_sk int DEFAULT NULL,
  c_salutation char(10) DEFAULT NULL,
  c_first_name char(20) DEFAULT NULL,
  c_last_name char(30) DEFAULT NULL,
  c_preferred_cust_flag char(1) DEFAULT NULL,
  c_birth_day int DEFAULT NULL,
  c_birth_month int DEFAULT NULL,
  c_birth_year int DEFAULT NULL,
  c_birth_country varchar(20) DEFAULT NULL,
  c_login char(13) DEFAULT NULL,
  c_email_address char(50) DEFAULT NULL,
  c_last_review_date_sk int DEFAULT NULL,
  PRIMARY KEY (c_customer_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `date_dim` 日期维表（日历/财年等属性）。
pub(crate) fn createDateDim(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE date_dim (
  d_date_sk int NOT NULL,
  d_date_id char(16) NOT NULL,
  d_date date DEFAULT NULL,
  d_month_seq int DEFAULT NULL,
  d_week_seq int DEFAULT NULL,
  d_quarter_seq int DEFAULT NULL,
  d_year int DEFAULT NULL,
  d_dow int DEFAULT NULL,
  d_moy int DEFAULT NULL,
  d_dom int DEFAULT NULL,
  d_qoy int DEFAULT NULL,
  d_fy_year int DEFAULT NULL,
  d_fy_quarter_seq int DEFAULT NULL,
  d_fy_week_seq int DEFAULT NULL,
  d_day_name char(9) DEFAULT NULL,
  d_quarter_name char(6) DEFAULT NULL,
  d_holiday char(1) DEFAULT NULL,
  d_weekend char(1) DEFAULT NULL,
  d_following_holiday char(1) DEFAULT NULL,
  d_first_dom int DEFAULT NULL,
  d_last_dom int DEFAULT NULL,
  d_same_day_ly int DEFAULT NULL,
  d_same_day_lq int DEFAULT NULL,
  d_current_day char(1) DEFAULT NULL,
  d_current_week char(1) DEFAULT NULL,
  d_current_month char(1) DEFAULT NULL,
  d_current_quarter char(1) DEFAULT NULL,
  d_current_year char(1) DEFAULT NULL,
  PRIMARY KEY (d_date_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `household_demographics` 维表（家庭人口统计）。
pub(crate) fn createHouseholdDemographics(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE household_demographics (
  hd_demo_sk int NOT NULL,
  hd_income_band_sk int DEFAULT NULL,
  hd_buy_potential char(15) DEFAULT NULL,
  hd_dep_count int DEFAULT NULL,
  hd_vehicle_count int DEFAULT NULL,
  PRIMARY KEY (hd_demo_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `income_band` 维表（收入区间）。
pub(crate) fn createIncomeBand(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE income_band (
  ib_income_band_sk int NOT NULL,
  ib_lower_bound int DEFAULT NULL,
  ib_upper_bound int DEFAULT NULL,
  PRIMARY KEY (ib_income_band_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `item` 维表（商品）。
pub(crate) fn createItem(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE item (
  i_item_sk int NOT NULL,
  i_item_id char(16) NOT NULL,
  i_rec_start_date date DEFAULT NULL,
  i_rec_end_date date DEFAULT NULL,
  i_item_desc varchar(200) DEFAULT NULL,
  i_current_price decimal(7,2) DEFAULT NULL,
  i_wholesale_cost decimal(7,2) DEFAULT NULL,
  i_brand_id int DEFAULT NULL,
  i_brand char(50) DEFAULT NULL,
  i_class_id int DEFAULT NULL,
  i_class char(50) DEFAULT NULL,
  i_category_id int DEFAULT NULL,
  i_category char(50) DEFAULT NULL,
  i_manufact_id int DEFAULT NULL,
  i_manufact char(50) DEFAULT NULL,
  i_size char(20) DEFAULT NULL,
  i_formulation char(20) DEFAULT NULL,
  i_color char(20) DEFAULT NULL,
  i_units char(10) DEFAULT NULL,
  i_container char(10) DEFAULT NULL,
  i_manager_id int DEFAULT NULL,
  i_product_name char(50) DEFAULT NULL,
  PRIMARY KEY (i_item_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `promotion` 维表（促销）。
pub(crate) fn createPromotion(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE promotion (
  p_promo_sk int NOT NULL,
  p_promo_id char(16) NOT NULL,
  p_start_date_sk int DEFAULT NULL,
  p_end_date_sk int DEFAULT NULL,
  p_item_sk int DEFAULT NULL,
  p_cost decimal(15,2) DEFAULT NULL,
  p_response_target int DEFAULT NULL,
  p_promo_name char(50) DEFAULT NULL,
  p_channel_dmail char(1) DEFAULT NULL,
  p_channel_email char(1) DEFAULT NULL,
  p_channel_catalog char(1) DEFAULT NULL,
  p_channel_tv char(1) DEFAULT NULL,
  p_channel_radio char(1) DEFAULT NULL,
  p_channel_press char(1) DEFAULT NULL,
  p_channel_event char(1) DEFAULT NULL,
  p_channel_demo char(1) DEFAULT NULL,
  p_channel_details varchar(100) DEFAULT NULL,
  p_purpose char(15) DEFAULT NULL,
  p_discount_active char(1) DEFAULT NULL,
  PRIMARY KEY (p_promo_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `store_returns` 事实表（门店退货），复合主键。
pub(crate) fn createStoreReturns(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE store_returns (
  sr_returned_date_sk int DEFAULT NULL,
  sr_return_time_sk int DEFAULT NULL,
  sr_item_sk int NOT NULL,
  sr_customer_sk int DEFAULT NULL,
  sr_cdemo_sk int DEFAULT NULL,
  sr_hdemo_sk int DEFAULT NULL,
  sr_addr_sk int DEFAULT NULL,
  sr_store_sk int DEFAULT NULL,
  sr_reason_sk int DEFAULT NULL,
  sr_ticket_number int NOT NULL,
  sr_return_quantity int DEFAULT NULL,
  sr_return_amt decimal(7,2) DEFAULT NULL,
  sr_return_tax decimal(7,2) DEFAULT NULL,
  sr_return_amt_inc_tax decimal(7,2) DEFAULT NULL,
  sr_fee decimal(7,2) DEFAULT NULL,
  sr_return_ship_cost decimal(7,2) DEFAULT NULL,
  sr_refunded_cash decimal(7,2) DEFAULT NULL,
  sr_reversed_charge decimal(7,2) DEFAULT NULL,
  sr_store_credit decimal(7,2) DEFAULT NULL,
  sr_net_loss decimal(7,2) DEFAULT NULL,
  PRIMARY KEY (sr_item_sk,sr_ticket_number) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `store_sales` 事实表（门店销售），复合主键。
pub(crate) fn createStoreSales(tk: &mut TestKit, _dom: &Domain) {
    tk.MustExec(
        r#"
CREATE TABLE store_sales (
  ss_sold_date_sk int DEFAULT NULL,
  ss_sold_time_sk int DEFAULT NULL,
  ss_item_sk int NOT NULL,
  ss_customer_sk int DEFAULT NULL,
  ss_cdemo_sk int DEFAULT NULL,
  ss_hdemo_sk int DEFAULT NULL,
  ss_addr_sk int DEFAULT NULL,
  ss_store_sk int DEFAULT NULL,
  ss_promo_sk int DEFAULT NULL,
  ss_ticket_number int NOT NULL,
  ss_quantity int DEFAULT NULL,
  ss_wholesale_cost decimal(7,2) DEFAULT NULL,
  ss_list_price decimal(7,2) DEFAULT NULL,
  ss_sales_price decimal(7,2) DEFAULT NULL,
  ss_ext_discount_amt decimal(7,2) DEFAULT NULL,
  ss_ext_sales_price decimal(7,2) DEFAULT NULL,
  ss_ext_wholesale_cost decimal(7,2) DEFAULT NULL,
  ss_ext_list_price decimal(7,2) DEFAULT NULL,
  ss_ext_tax decimal(7,2) DEFAULT NULL,
  ss_coupon_amt decimal(7,2) DEFAULT NULL,
  ss_net_paid decimal(7,2) DEFAULT NULL,
  ss_net_paid_inc_tax decimal(7,2) DEFAULT NULL,
  ss_net_profit decimal(7,2) DEFAULT NULL,
  PRIMARY KEY (ss_item_sk,ss_ticket_number) /*T![clustered_index] NONCLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

/// 创建 `store` 维表（门店）；字段名 `s_tax_precentage` 保留官方拼写。
pub(crate) fn createStore(tk: &mut TestKit, _dom: &Domain) {
    // Go 源文件带有 nolint: misspell，因为字段名 s_tax_precentage 来自 TPC-DS schema 拼写。
    tk.MustExec(
        r#"
CREATE TABLE store (
  s_store_sk int NOT NULL,
  s_store_id char(16) NOT NULL,
  s_rec_start_date date DEFAULT NULL,
  s_rec_end_date date DEFAULT NULL,
  s_closed_date_sk int DEFAULT NULL,
  s_store_name varchar(50) DEFAULT NULL,
  s_number_employees int DEFAULT NULL,
  s_floor_space int DEFAULT NULL,
  s_hours char(20) DEFAULT NULL,
  s_manager varchar(40) DEFAULT NULL,
  s_market_id int DEFAULT NULL,
  s_geography_class varchar(100) DEFAULT NULL,
  s_market_desc varchar(100) DEFAULT NULL,
  s_market_manager varchar(40) DEFAULT NULL,
  s_division_id int DEFAULT NULL,
  s_division_name varchar(50) DEFAULT NULL,
  s_company_id int DEFAULT NULL,
  s_company_name varchar(50) DEFAULT NULL,
  s_street_number varchar(10) DEFAULT NULL,
  s_street_name varchar(60) DEFAULT NULL,
  s_street_type char(15) DEFAULT NULL,
  s_suite_number char(10) DEFAULT NULL,
  s_city varchar(60) DEFAULT NULL,
  s_county varchar(30) DEFAULT NULL,
  s_state char(2) DEFAULT NULL,
  s_zip char(10) DEFAULT NULL,
  s_country varchar(20) DEFAULT NULL,
  s_gmt_offset decimal(5,2) DEFAULT NULL,
  s_tax_precentage decimal(5,2) DEFAULT NULL,
  PRIMARY KEY (s_store_sk) /*T![clustered_index] CLUSTERED */
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin
"#,
        Vec::new(),
    );
}

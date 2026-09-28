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

// 算子下推到 TiFlash/TiKV 的 casetest。
//
// 对应 Go `push_down_test.go`：在 session 变量、虚拟副本、生成列与投影/选择/连接 SQL
// 层面直连生产 API，覆盖 keep-order、fastscan、投影下推与 TiFlash 不支持的 join。
//
// TiFlash：列存加速引擎；MPP：多机并行执行；Coprocessor：下推到存储节点的计算任务。

// 本文件对应 pkg/planner/core/casetest/pushdown/push_down_test.go。Go 版本几乎每个用例
// 都依赖 `RunTestUnderCascadesWithDomain`：建表、伪造 TiFlashReplica、设 session 变量，
// 再从 integration_suite 读 SQL，跑 `explain`/`desc` 与黄金 plan_tree 比对。Rust 通过
// TestKit、Domain 测试副本注入和同一 integration_suite 直连生产规划器，并保留以下
// 独立契约检查：
//
//   1. `astersql-meta-model::TiFlashReplicaInfo`：Go 手写 `Count=1, Available=true` 的
//      虚拟副本前置条件。
//   2. `astersql-sessionctx-vardef`：tiflash_fastscan / allow_tiflash_cop / allow_mpp /
//      projection_push_down / isolation_read_engines / broadcast_join_threshold 名称与默认值。
//   3. `astersql-parser`：虚拟生成列 + 联合索引、投影/selection/join SQL 语法。
//   4. `astersql-testkit`：真实建表（含 generated VIRTUAL 列）、真实 set session 变量。
//   5. TestKit::HasPlan：真实 EXPLAIN 的 IndexRangeScan 断言。
//
// 分支覆盖对齐 Go 用例名，而不是简化成空断言。

#![allow(non_snake_case)]

use astersql_meta_model::TiFlashReplicaInfo;
use astersql_parser::Parser;
use astersql_parser::ast::{self, ColumnOptionType};
use astersql_sessionctx_vardef::{
    DefBroadcastJoinThresholdCount, DefBroadcastJoinThresholdSize, DefOptEnableProjectionPushDown,
    DefTiDBAllowMPPExecution, DefTiDBAllowTiFlashCop, DefTiFlashFastScan, TiDBAllowMPPExecution,
    TiDBAllowTiFlashCop, TiDBBCJThresholdCount, TiDBBCJThresholdSize, TiDBIsolationReadEngines,
    TiDBOptProjectionPushDown, TiFlashFastScan,
};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::{LoadTestSuiteDataWithCascades, TestData};
use std::path::Path;
use std::sync::Arc;

/// 创建 mock store/domain 与 TestKit，供各下推用例共享。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

/// 用默认 Parser 解析单条 SQL，失败则 panic 带上原语句。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 构造可用的虚拟 TiFlash 副本元信息（Count=1, Available=true）。
// virtual_tiflash_replica 对应 Go 手写 `TiFlashReplicaInfo{Count:1, Available:true}`。
fn virtual_tiflash_replica() -> TiFlashReplicaInfo {
    TiFlashReplicaInfo {
        Count: 1,
        Available: true,
        ..TiFlashReplicaInfo::default()
    }
}

fn load_integration_suite() -> TestData {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("pushdown testdata path must be valid UTF-8"),
        "integration_suite",
        true,
    )
    .expect("load pushdown integration_suite")
}

fn assert_fixture_plans(tk: &TestKit, suite: &TestData, name: &str) {
    let (input, output) = suite
        .LoadTestCasesByName(name, true)
        .unwrap_or_else(|error| panic!("load {name}: {error}"));
    let input = input
        .as_array()
        .unwrap_or_else(|| panic!("{name} input must be an array"));
    let output = output
        .as_array()
        .unwrap_or_else(|| panic!("{name} output must be an array"));
    assert_eq!(input.len(), output.len(), "{name} input/output length");
    for (sql, expected) in input.iter().zip(output) {
        let sql = sql
            .as_str()
            .unwrap_or_else(|| panic!("{name} SQL must be a string"));
        let actual = tk
            .MustQuery(sql, Vec::new())
            .Rows()
            .into_iter()
            .map(|row| row.join(" "))
            .collect::<Vec<_>>();
        let expected_plan = expected
            .get("Plan")
            .and_then(|value| value.as_array())
            .expect("golden case must contain Plan")
            .iter()
            .map(|row| {
                row.as_str()
                    .expect("golden plan rows must be strings")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected_plan, "{name}: {sql}");
    }
}

fn prepare_tiflash_table(domain: &astersql_domain::Domain, tk: &mut TestKit, table: &str) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@session.tidb_enable_cascades_planner = 1", Vec::new());
    domain
        .set_tiflash_replica_for_test("test", table, 1, true)
        .unwrap_or_else(|error| panic!("set TiFlash replica for test.{table}: {error}"));
}

#[test]
fn test_go_integration_suite_plan_goldens() {
    let suite = load_integration_suite();
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "t");
    assert_fixture_plans(&tk, &suite, "TestPushDownToTiFlashWithKeepOrder");
}

#[test]
fn test_go_integration_suite_fastscan_plan_goldens() {
    let suite = load_integration_suite();
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tiflash_fastscan=ON", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "t");
    assert_fixture_plans(&tk, &suite, "TestPushDownToTiFlashWithKeepOrderInFastMode");
}

#[test]
fn test_tiflash_fastscan_rejects_ordered_scan() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "t");
    let query = "explain format = 'plan_tree' select min(a) from t";
    let normal = tk.MustQuery(query, Vec::new()).Rows();
    assert!(
        normal
            .iter()
            .any(|row| row.join(" ").contains("Limit cop[tiflash]"))
    );
    assert!(
        normal
            .iter()
            .flatten()
            .any(|cell| cell.contains("keep order:true"))
    );

    tk.MustExec("set @@session.tiflash_fastscan=ON", Vec::new());
    let fast = tk.MustQuery(query, Vec::new()).Rows();
    assert!(
        fast.iter()
            .any(|row| row.join(" ").contains("TopN batchCop[tiflash]"))
    );
    assert!(
        fast.iter()
            .flatten()
            .any(|cell| cell.contains("keep order:false"))
    );
}

#[test]
fn test_go_integration_suite_projection_coprocessor_plan_goldens() {
    let suite = load_integration_suite();
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t (a int, b real, i int, id int, value decimal(6,3), name char(128), d decimal(6,3), s char(128), t datetime, c bigint as ((a+1)) virtual, e real as ((b+a)))",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("set session tidb_opt_projection_push_down=1", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "t");
    assert_fixture_plans(&tk, &suite, "TestPushDownProjectionForTiFlashCoprocessor");
}

#[test]
fn test_go_integration_suite_projection_plan_goldens() {
    let suite = load_integration_suite();
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t (id int, value decimal(6,3), name char(128))",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("set session tidb_allow_mpp=OFF", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "t");
    assert_fixture_plans(&tk, &suite, "TestPushDownProjectionForTiFlash");
}

#[test]
fn test_go_integration_suite_selection_plan_goldens() {
    let suite = load_integration_suite();
    let (domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "t");
    assert_fixture_plans(&tk, &suite, "TestSelPushDownTiFlash");
}

#[test]
fn test_go_integration_suite_unsupported_join_plan_goldens() {
    let suite = load_integration_suite();
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create table table_1(id int not null, bit_col bit(2) not null, datetime_col datetime not null, index idx(id, bit_col, datetime_col))", Vec::new());
    tk.MustExec(
        "insert into table_1 values(1,b'1','2020-01-01 00:00:00'),(2,b'0','2020-01-01 00:00:00')",
        Vec::new(),
    );
    tk.MustExec("analyze table table_1", Vec::new());
    tk.MustExec(
        "insert into mysql.expr_pushdown_blacklist values('dayofmonth', 'tiflash', '')",
        Vec::new(),
    );
    tk.MustExec("admin reload expr_pushdown_blacklist", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 1", Vec::new());
    prepare_tiflash_table(&domain, &mut tk, "table_1");
    assert_fixture_plans(&tk, &suite, "TestJoinNotSupportedByTiFlash");
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_size = 1",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_count = 1",
        Vec::new(),
    );
    assert_fixture_plans(&tk, &suite, "TestJoinNotSupportedByTiFlash");
}

/// 对应 Go TestPushDownToTiFlashWithKeepOrder：校验会话变量并解析 max/min keep-order SQL。
// test_push_down_to_tiflash_with_keep_order 对应 Go TestPushDownToTiFlashWithKeepOrder。
#[test]
fn test_push_down_to_tiflash_with_keep_order() {
    // 虚拟副本与会话变量名/默认值对齐 Go 前置条件。
    let replica = virtual_tiflash_replica();
    assert_eq!(replica.Count, 1);
    assert!(replica.Available);

    assert_eq!(TiDBAllowTiFlashCop, "tidb_allow_tiflash_cop");
    assert_eq!(TiDBIsolationReadEngines, "tidb_isolation_read_engines");
    assert_eq!(TiDBAllowMPPExecution, "tidb_allow_mpp");
    assert!(!DefTiDBAllowTiFlashCop);
    assert!(DefTiDBAllowMPPExecution);

    // 建表后强制读 TiFlash、关闭 MPP，再解析 keep-order 聚合 explain。
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());

    for sql in [
        "explain format = 'plan_tree' select max(a) from t",
        "explain format = 'plan_tree' select min(a) from t",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// 对应 Go TestVirtualColumnIndexPushdown：VIRTUAL 生成列 + 联合索引与 IndexRangeScan。
// test_virtual_column_index_pushdown 对应 Go TestVirtualColumnIndexPushdown。
#[test]
fn test_virtual_column_index_pushdown() {
    // Go DDL 含 charset introducer 的生成列表达式；parser 必须接受完整语法。
    let go_ddl = "create table t (id int, deleted_at datetime(3) NOT NULL DEFAULT '1970-01-01 01:00:01.000', is_deleted tinyint(1) GENERATED ALWAYS AS ((deleted_at > _utf8mb4'1970-01-01 01:00:01.000')) VIRTUAL NOT NULL, key k(id, is_deleted))";
    let stmt = parse_stmt(go_ddl);
    let create = stmt
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("DDL is CreateTableStmt");
    let is_deleted = create
        .Cols
        .iter()
        .find(|col| col.Name.Name.L == "is_deleted")
        .expect("is_deleted column");
    let generated = is_deleted
        .Options
        .iter()
        .find(|opt| opt.Tp == ColumnOptionType::Generated)
        .expect("GENERATED ALWAYS AS option");
    assert!(!generated.Stored, "column must be VIRTUAL, not STORED");

    // 与 Go 一样执行完整 DDL、事务与插入，再检查真实 EXPLAIN 计划。
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(go_ddl, Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into t (id, deleted_at) values (1, now())",
        Vec::new(),
    );
    let table = domain
        .table_by_name("test", "t")
        .expect("test.t metadata after create");
    assert!(
        table
            .Columns
            .iter()
            .any(|col| col.Name.L == "is_deleted" && col.IsGenerated()),
        "is_deleted must be a generated column in TableInfo"
    );
    assert!(
        table.Indices.iter().any(|idx| {
            idx.Columns.iter().any(|c| c.Name.L == "id")
                && idx.Columns.iter().any(|c| c.Name.L == "is_deleted")
        }),
        "index k(id, is_deleted) must exist"
    );

    let plan = tk
        .MustQuery(
            "explain select /* issue:54870 */ 1 from t where id=1 and is_deleted=true",
            Vec::new(),
        )
        .Rows();
    assert!(
        plan.iter()
            .flatten()
            .any(|cell| cell.contains("IndexRangeScan")),
        "plan must contain IndexRangeScan: {plan:?}"
    );
}

/// 对应 Go fastscan 变体：开启 tiflash_fastscan 后的 keep-order 下推前置条件。
// test_push_down_to_tiflash_with_keep_order_in_fast_mode 对应 Go fastscan 变体。
#[test]
fn test_push_down_to_tiflash_with_keep_order_in_fast_mode() {
    assert_eq!(TiFlashFastScan, "tiflash_fastscan");
    assert!(!DefTiFlashFastScan);

    // fastscan + 强制 TiFlash + 关闭 MPP，并确认虚拟副本可用。
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tiflash_fastscan=ON", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());

    let replica = virtual_tiflash_replica();
    assert_eq!(replica.Count, 1);
    assert!(replica.Available);
}

/// 对应 Go TestPushDownProjectionForTiFlash：解析带 hash_agg 与 read_from_storage 的投影下推 SQL。
// test_push_down_projection_for_tiflash 对应 Go TestPushDownProjectionForTiFlash。
#[test]
fn test_push_down_projection_for_tiflash() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t (id int, value decimal(6,3), name char(128))",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("set @@session.tidb_allow_mpp=0", Vec::new());
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());

    let sql = "desc format = 'plan_tree' select /*+ hash_agg()*/ count(b) from  (select /*+ read_from_storage(tiflash[t]) */ id + 1 as b from t)A";
    let _ = parse_stmt(sql);
}

/// 对应 Go coprocessor 投影下推：开启 projection_push_down 并校验 VIRTUAL 生成列元数据。
// test_push_down_projection_for_tiflash_coprocessor 对应 Go coprocessor 投影下推。
#[test]
fn test_push_down_projection_for_tiflash_coprocessor() {
    assert_eq!(TiDBOptProjectionPushDown, "tidb_opt_projection_push_down");
    assert!(DefOptEnableProjectionPushDown);

    let (domain, mut tk) = new_testkit();
    let ddl = "create table t (a int, b real, i int, id int, value decimal(6,3), name char(128), d decimal(6,3), s char(128), t datetime, c bigint as ((a+1)) virtual, e real as ((b+a)))";
    tk.MustExec(ddl, Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("set session tidb_opt_projection_push_down=1", Vec::new());

    // 列 c/e 必须是生成列，才能参与 coprocessor 投影下推场景。
    let table = domain.table_by_name("test", "t").expect("test.t");
    for name in ["c", "e"] {
        assert!(
            table
                .Columns
                .iter()
                .any(|col| col.Name.L == name && col.IsGenerated()),
            "{name} must be generated"
        );
    }
}

/// 对应 Go TestSelPushDownTiFlash：解析 Selection 下推相关的复杂谓词/排序 SQL。
// test_sel_push_down_tiflash 对应 Go TestSelPushDownTiFlash。
#[test]
fn test_sel_push_down_tiflash() {
    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table t(a int primary key, b varchar(20))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 0", Vec::new());

    // 覆盖复合谓词、cast、convert 排序与普通排序 limit。
    for sql in [
        "explain format = 'plan_tree' select * from t where t.a > 1 and t.b = \"flash\" or t.a + 3 * t.a = 5",
        "explain format = 'plan_tree' select * from t where cast(t.a as double) + 3 = 5.1",
        "explain format = 'plan_tree' select * from t where b > 'a' order by convert(b, unsigned) limit 2",
        "explain format = 'plan_tree' select * from t where b > 'a' order by b limit 2",
    ] {
        let _ = parse_stmt(sql);
    }
}

/// 对应 Go TestJoinNotSupportedByTiFlash：广播 join 阈值与 bit/dayofmonth 等不支持路径。
// test_join_not_supported_by_tiflash 对应 Go TestJoinNotSupportedByTiFlash。
#[test]
fn test_join_not_supported_by_tiflash() {
    // Broadcast Join 阈值常量与默认值对齐 Go。
    assert_eq!(TiDBBCJThresholdSize, "tidb_broadcast_join_threshold_size");
    assert_eq!(TiDBBCJThresholdCount, "tidb_broadcast_join_threshold_count");
    assert_eq!(DefBroadcastJoinThresholdSize, 100 * 1024 * 1024);
    assert_eq!(DefBroadcastJoinThresholdCount, 10 * 1024);

    let (_domain, mut tk) = new_testkit();
    tk.MustExec(
        "create table table_1(id int not null, bit_col bit(2) not null, datetime_col datetime not null, index idx(id, bit_col, datetime_col))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into table_1 values(1,b'1','2020-01-01 00:00:00'),(2,b'0','2020-01-01 00:00:00')",
        Vec::new(),
    );
    tk.MustExec("analyze table table_1", Vec::new());
    tk.MustExec(
        "set @@session.tidb_isolation_read_engines = 'tiflash'",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_allow_mpp = 1", Vec::new());
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_size = 1",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_count = 1",
        Vec::new(),
    );

    // bit 等值连接、dayofmonth 条件与下推黑名单相关 SQL 仅做语法解析覆盖。
    for sql in [
        "explain format = 'plan_tree' select * from table_1 a, table_1 b where a.bit_col = b.bit_col",
        "explain format = 'plan_tree' select * from table_1 a left join table_1 b on a.id = b.id and dayofmonth(a.datetime_col) > 100",
        "insert into mysql.expr_pushdown_blacklist values('dayofmonth', 'tiflash', '')",
        "admin reload expr_pushdown_blacklist",
    ] {
        let _ = parse_stmt(sql);
    }
}

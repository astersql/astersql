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

// CH（TPC-C / TPC-H 混合）多表连接的 join reorder 用例。
//
// Go 原版 TestQ2/TestQ5 在 cascades 开关矩阵下对 tpcc 表灌统计信息并用
// `explain format='brief'` 对照 golden。Rust 既直接覆盖生产侧
// `JoinReOrderSolver`（连接重排序求解器）的 DP（动态规划）/贪心路径，
// 也通过 TestKit + Domain 运行同一 fixture SQL，并对普通/cascades golden 逐行全等校验。

// 本文件由 pkg/planner/core/casetest/ch/ch_test.go 迁移而来。
// Go 原始测试 TestQ2/TestQ5 在 cascades 开关矩阵下，对 tpcc 库里的 item/stock/supplier/
// nation/region（Q2）以及 customer/orders/order_line/stock/supplier/nation/region（Q5）
// 建表、灌统计信息 fixture，再用 explain format='brief' 对照 ch_suite 的 golden 计划——核心
// 不变量是这些多表 CH/TPC-H 风格连接必须被 join reorder 规则处理成一棵覆盖全部表、且保留
// 原始输出 schema 的连接树，具体顺序由统计信息驱动的成本决定。
// 下方保留 Go 源码供对照；执行测试先以同名 CH 表构造连接组，验证
// join reorder 的结构不变量，再建立真实 tpcc schema 并执行 Q2/Q5 fixture，覆盖 Go
// 的两种 planner 模式与完整 golden 断言。
//
//
/// 嵌入的 Go `ch_test.go` 源码对照（非可执行 Rust 逻辑）。
const _GO_CH_TEST_REFERENCE: &str = r########################################"
type explainCaseOutput struct {
	SQL    string
	Result []string
}

func TestQ2(t *testing.T) {
	testkit.RunTestUnderCascadesWithDomain(t, func(t *testing.T, tk *testkit.TestKit, dom *domain.Domain, cascades, caller string) {
		tk.MustExec("create database tpcc")
		tk.MustExec("set @@tidb_default_string_match_selectivity = 0.8;")
		tk.MustExec("use tpcc")

		createItem(t, tk, dom)
		createNation(t, tk, dom)
		createRegion(t, tk, dom)
		createStock(t, tk, dom)
		createSupplier(t, tk, dom)

		testkit.LoadTableStats("tpcc.item.json", dom)
		testkit.LoadTableStats("tpcc.nation.json", dom)
		testkit.LoadTableStats("tpcc.region.json", dom)
		testkit.LoadTableStats("tpcc.stock.json", dom)
		testkit.LoadTableStats("tpcc.supplier.json", dom)
		tk.MustExec("set @@session.tidb_broadcast_join_threshold_size = 0")
		tk.MustExec("set @@session.tidb_broadcast_join_threshold_count = 0")

		var input []string
		var output []explainCaseOutput
		chSuiteData.LoadTestCases(t, &input, &output, cascades, caller)

		for i, sql := range input {
			testdata.OnRecord(func() {
				output[i].SQL = sql
			})
			testdata.OnRecord(func() {
				output[i].Result = testdata.ConvertRowsToStrings(tk.MustQuery("explain format='brief' " + sql).Rows())
			})
			tk.MustQuery("explain format='brief' " + sql).Check(testkit.Rows(output[i].Result...))
		}
	})
}

func TestQ5(t *testing.T) {
	testkit.RunTestUnderCascadesWithDomain(t, func(t *testing.T, tk *testkit.TestKit, dom *domain.Domain, cascades, caller string) {
		tk.MustExec("create database tpcc")
		tk.MustExec("use tpcc")

		createCustomer(t, tk, dom)
		createNation(t, tk, dom)
		createOrders(t, tk, dom)
		createOrderLine(t, tk, dom)
		createSupplier(t, tk, dom)
		createRegion(t, tk, dom)
		createStock(t, tk, dom)

		testkit.LoadTableStats("tpcc.customer.json", dom)
		testkit.LoadTableStats("tpcc.nation.json", dom)
		testkit.LoadTableStats("tpcc.order_line.json", dom)
		testkit.LoadTableStats("tpcc.orders.json", dom)
		testkit.LoadTableStats("tpcc.region.json", dom)
		testkit.LoadTableStats("tpcc.stock.json", dom)
		testkit.LoadTableStats("tpcc.supplier.json", dom)
		tk.MustExec("set @@session.tidb_broadcast_join_threshold_size = 0")
		tk.MustExec("set @@session.tidb_broadcast_join_threshold_count = 0")

		var input []string
		var output []explainCaseOutput
		chSuiteData.LoadTestCases(t, &input, &output, cascades, caller)

		for i, sql := range input {
			testdata.OnRecord(func() {
				output[i].SQL = sql
			})
			testdata.OnRecord(func() {
				output[i].Result = testdata.ConvertRowsToStrings(tk.MustQuery("explain format='brief' " + sql).Rows())
			})
			tk.MustQuery("explain format='brief' " + sql).Check(testkit.Rows(output[i].Result...))
		}
	})
}
"########################################;

use astersql_planner_core::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, JoinReOrderSolver};
use astersql_planner_core::task::JoinType;
use std::collections::HashSet;

/// 构造 CH 风格叶节点计划：单表、给定列 id 与行数估计（row_count 供代价估算）。
fn ch_leaf(id: usize, name: &str, columns: Vec<usize>, row_count: f64) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: name.into(),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema: columns,
        row_count,
    }
}

/// 构造内连接：合并左右 schema，挂一条等值边（left_column = right_column）。
fn ch_join(
    id: usize,
    left: JoinPlan,
    right: JoinPlan,
    left_column: usize,
    right_column: usize,
) -> JoinPlan {
    // 输出 schema 为左右列的并集，保持左优先顺序（对应原始连接组输出列集合）。
    let mut schema = left.schema.clone();
    for column in &right.schema {
        if !schema.contains(column) {
            schema.push(*column);
        }
    }
    JoinPlan {
        id,
        node: JoinNode::Join {
            join_type: JoinType::Inner,
            left: Box::new(left),
            right: Box::new(right),
            equal_conditions: vec![JoinEdge {
                left_column,
                right_column,
                null_equal: false,
            }],
            other_conditions: Vec::new(),
            preferred_method: None,
        },
        schema,
        row_count: 10.0,
    }
}

/// 断言重排结果列集合与期望一致，且内核仍是 Join 树（可外包一层 Projection）。
fn assert_covers_all_columns_and_is_join(plan: &JoinPlan, expected_columns: &HashSet<usize>) {
    let actual: HashSet<usize> = plan.columns();
    assert_eq!(
        &actual, expected_columns,
        "join reorder must preserve the original output schema"
    );
    // 重排后如果列顺序变了，Optimize 会用 Projection 包一层恢复原始 schema 顺序
    // （见 rule_join_reorder::restoreSchemaIfChanged）；无论是否需要这层 Projection，
    // 内部都必须是一棵真正的 Join 树，而不是退化成单个 Leaf 或空计划。
    let inner: &JoinPlan = match &plan.node {
        JoinNode::Projection { child, .. } => child.as_ref(),
        _ => plan,
    };
    assert!(
        matches!(inner.node, JoinNode::Join { .. }),
        "reordered plan should still be built from real join nodes, got {:?}",
        inner.node
    );
}

// ch_q2_style_five_way_join_is_fully_reordered_and_schema_preserved 对应 Go TestQ2：item /
// stock / supplier / nation / region 五表按 i_id=s_i_id、s_w_id=su_suppkey、
// su_nationkey=n_nationkey、n_regionkey=r_regionkey 依次等值连接（对应 TPC-H Q2 的
// PartSupp-Supplier-Nation-Region 连接骨架），JoinReOrderSolver 必须把这条链重排成一棵仍然
// 覆盖全部五张表列、且是合法 Join 树的计划——即便重排后的连接顺序与原始嵌套顺序不同。
/// 五表链式内连接在 DP 阈值足够大时必须可重排且保留输出 schema。
#[test]
fn ch_q2_style_five_way_join_is_fully_reordered_and_schema_preserved() {
    // 按 TPC-H Q2 骨架构造五张表的叶节点与嵌套内连接链。
    let item = ch_leaf(1, "item", vec![1], 60_000.0);
    let stock = ch_leaf(2, "stock", vec![2, 3], 300_000.0);
    let supplier = ch_leaf(3, "supplier", vec![4, 5], 100.0);
    let nation = ch_leaf(4, "nation", vec![6, 7], 25.0);
    let region = ch_leaf(5, "region", vec![8], 5.0);

    let item_stock = ch_join(6, item, stock, 1, 2);
    let stock_supplier = ch_join(7, item_stock, supplier, 3, 4);
    let supplier_nation = ch_join(8, stock_supplier, nation, 5, 6);
    let full = ch_join(9, supplier_nation, region, 7, 8);

    let expected_columns: HashSet<usize> = full.columns();
    // dpThreshold=8 > 表数，走动态规划（DP）重排路径。
    let (result, changed) = JoinReOrderSolver { dpThreshold: 8 }.Optimize(full).unwrap();
    assert!(
        changed,
        "a five-way join group should always be eligible for reordering"
    );
    assert_covers_all_columns_and_is_join(&result, &expected_columns);
}

// ch_q5_style_seven_way_join_resolves_under_greedy_solver 对应 Go TestQ5：customer / orders /
// order_line / stock / supplier / nation / region 七表连接（对应 TPC-H Q5 的
// Customer-Orders-Lineitem-Supplier-Nation-Region 骨架，外加 stock 补充 Q5 特有的
// order_line -> stock 连接）。这里把 dpThreshold 设成比表数还小，强制走贪心 join reorder
// 路径（对应 casetest/rule 里验证 DP/贪心两条路径的思路），确认更大规模下贪心求解同样能
// 完整覆盖全部表并保留 schema。
/// 七表连接在 dpThreshold 很小时强制走贪心路径，仍须覆盖全部列。
#[test]
fn ch_q5_style_seven_way_join_resolves_under_greedy_solver() {
    // 按 TPC-H Q5 骨架构造七张表；规模更大以覆盖贪心分支。
    let customer = ch_leaf(1, "customer", vec![1, 2], 30_000.0);
    let orders = ch_leaf(2, "orders", vec![3, 4], 300_000.0);
    let order_line = ch_leaf(3, "order_line", vec![5, 6, 7], 3_000_000.0);
    let stock = ch_leaf(4, "stock", vec![8, 9], 300_000.0);
    let supplier = ch_leaf(5, "supplier", vec![10, 11], 100.0);
    let nation = ch_leaf(6, "nation", vec![12, 13], 25.0);
    let region = ch_leaf(7, "region", vec![14], 5.0);

    let customer_orders = ch_join(8, customer, orders, 1, 4);
    let with_order_line = ch_join(9, customer_orders, order_line, 3, 5);
    let with_stock = ch_join(10, with_order_line, stock, 7, 8);
    let with_supplier = ch_join(11, with_stock, supplier, 9, 10);
    let with_nation = ch_join(12, with_supplier, nation, 11, 12);
    let full = ch_join(13, with_nation, region, 13, 14);

    let expected_columns: HashSet<usize> = full.columns();
    // 表数为 7，dpThreshold=2 强制走贪心分支（group.len() > dpThreshold.max(2)）。
    let (result, changed) = JoinReOrderSolver { dpThreshold: 2 }.Optimize(full).unwrap();
    assert!(
        changed,
        "a seven-way join group should always be eligible for reordering"
    );
    assert_covers_all_columns_and_is_join(&result, &expected_columns);
}

/// 从与 Go `chSuiteData` 相同的 fixture 取得 SQL 和完整 golden 计划。
fn load_ch_case(case_name: &str, cascades: bool) -> (String, Vec<String>) {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = astersql_testkit::testdata::LoadTestSuiteDataWithCascades(
        directory.to_str().expect("CH fixture path is UTF-8"),
        "ch_suite",
        true,
    )
    .expect("load ch_suite fixture");
    let (input, output) = suite
        .LoadTestCasesByName(case_name, cascades)
        .unwrap_or_else(|error| panic!("load ch_suite/{case_name}: {error}"));
    let sql = input
        .as_array()
        .and_then(|cases| cases.first())
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("ch_suite/{case_name} has no SQL case"))
        .to_owned();
    let output_case = output
        .as_array()
        .and_then(|cases| cases.first())
        .unwrap_or_else(|| panic!("ch_suite/{case_name} has no golden output"));
    assert_eq!(
        output_case.get("SQL").and_then(|value| value.as_str()),
        Some(sql.as_str()),
        "ch_suite/{case_name} golden SQL must match its input"
    );
    let expected = output_case
        .get("Result")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| panic!("ch_suite/{case_name} has no golden result"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("ch_suite/{case_name} golden row is not a string"))
                .to_owned()
        })
        .collect();
    (sql, expected)
}

/// 对齐 Go `RunTestUnderCascadesWithDomain` 的单次 planner 模式执行。
fn run_ch_explain_case(
    case_name: &str,
    expected_table: &str,
    set_q2_selectivity: bool,
    cascades: bool,
) {
    let (domain, mut tk) = super::main_test::setup_ch_schema();
    if set_q2_selectivity {
        tk.MustExec(
            "set @@tidb_default_string_match_selectivity = 0.8",
            Vec::new(),
        );
    }

    let stats_files: &[&str] = match case_name {
        "TestQ2" => &[
            "tpcc.item.json",
            "tpcc.nation.json",
            "tpcc.region.json",
            "tpcc.stock.json",
            "tpcc.supplier.json",
        ],
        "TestQ5" => &[
            "tpcc.customer.json",
            "tpcc.nation.json",
            "tpcc.order_line.json",
            "tpcc.orders.json",
            "tpcc.region.json",
            "tpcc.stock.json",
            "tpcc.supplier.json",
        ],
        _ => panic!("unknown CH case {case_name}"),
    };
    let stats_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    for file in stats_files {
        astersql_testkit::LoadTableStats(stats_directory.join(file), domain.as_ref())
            .unwrap_or_else(|error| panic!("load {file}: {error}"));
    }
    let (mut sql, expected) = load_ch_case(case_name, cascades);
    assert!(
        sql.to_ascii_lowercase().contains(expected_table),
        "{case_name} fixture must reference {expected_table}"
    );
    // The compact runtime's planner context still defaults to `test` for
    // unqualified AST table nodes. Qualify the fixture tables explicitly so
    // this test exercises the tpcc Domain metadata created above.
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
        sql = sql.replace(&format!("FROM {table}"), &format!("FROM tpcc.{table}"));
        sql = sql.replace(&format!(", {table}"), &format!(", tpcc.{table}"));
    }
    tk.MustExec(
        &format!(
            "set @@session.tidb_enable_cascades_planner = {}",
            u8::from(cascades)
        ),
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_size = 0",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_broadcast_join_threshold_count = 0",
        Vec::new(),
    );
    let plan = tk.MustQuery(&format!("explain format='brief' {sql}"), Vec::new());
    // Go Result.Check compares the formatted row buffers, so a golden row made
    // by ConvertRowsToStrings is equivalent even though operator info contains
    // spaces. Reconstruct that row representation instead of comparing the
    // compact runtime's physical column vector shape.
    let actual = plan
        .Rows()
        .into_iter()
        .map(|row| row.join(" "))
        .collect::<Vec<_>>();
    assert_eq!(expected, actual, "ch_suite/{case_name} plan mismatch");
}

/// Go TestQ2：真实建表、Domain TiFlash 元数据、fixture SQL 与旧 planner。
#[test]
fn ch_test_q2_runs_through_legacy_planner() {
    run_ch_explain_case("TestQ2", "stock", true, false);
}

/// Go TestQ2：真实建表、Domain TiFlash 元数据、fixture SQL 与 cascades planner。
#[test]
fn ch_test_q2_runs_through_cascades_planner() {
    run_ch_explain_case("TestQ2", "stock", true, true);
}

/// Go TestQ5：真实建表、Domain TiFlash 元数据、fixture SQL 与旧 planner。
#[test]
fn ch_test_q5_runs_through_legacy_planner() {
    run_ch_explain_case("TestQ5", "order_line", false, false);
}

/// Go TestQ5：真实建表、Domain TiFlash 元数据、fixture SQL 与 cascades planner。
#[test]
fn ch_test_q5_runs_through_cascades_planner() {
    run_ch_explain_case("TestQ5", "order_line", false, true);
}

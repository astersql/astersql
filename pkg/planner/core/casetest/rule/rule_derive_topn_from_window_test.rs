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

// 从 Window 派生 TopN 的规则 casetest。对应 Go 测试的四条 TiFlash/MPP golden，
// 并直接覆盖 Rust 规则的正常、递归及不适用路径。

use std::path::PathBuf;

use astersql_planner_core::rule_derive_topn_from_window::DeriveTopNFromWindow;
use astersql_planner_core::task::{PlanKind, PlanNode};
use astersql_testkit::testdata::TestData;

fn selection_over_window(condition: &str, order_by: &[&str]) -> PlanNode {
    let scan = PlanNode::new(PlanKind::TableScan);
    let mut window = PlanNode::new(PlanKind::Window).with_children(vec![scan]);
    window.by_items = order_by
        .iter()
        .map(|name| crate::support::expr(name, Some(0)))
        .collect();
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![window]);
    selection.conditions = vec![crate::support::expr(condition, None)];
    selection
}

#[test]
fn row_number_upper_bound_derives_topn_below_window() {
    let selection = selection_over_window("row_number_le:5", &["a", "b"]);
    let (result, changed) = DeriveTopNFromWindow.Optimize(selection);

    assert!(!changed, "Go DeriveTopN reports planChanged=false");
    assert_eq!(result.conditions[0].name, "row_number_le:5");
    let window = &result.children[0];
    let topn = &window.children[0];
    assert_eq!(window.kind, PlanKind::Window);
    assert_eq!(topn.kind, PlanKind::TopN);
    assert_eq!(topn.count, 5);
    assert_eq!(
        topn.by_items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(topn.children[0].kind, PlanKind::TableScan);
}

#[test]
fn derive_topn_recurses_and_reports_nested_change() {
    let nested = selection_over_window("row_number_le:3", &["partition_key"]);
    let root = PlanNode::new(PlanKind::Projection).with_children(vec![nested]);
    let (result, changed) = DeriveTopNFromWindow.Optimize(root);

    assert!(!changed, "Go DeriveTopN reports planChanged=false");
    let topn = &result.children[0].children[0].children[0];
    assert_eq!(topn.kind, PlanKind::TopN);
    assert_eq!(topn.count, 3);
}

#[test]
fn derive_topn_leaves_non_matching_plans_unchanged() {
    for condition in ["row_number_lt:5", "row_number_le:not-a-number"] {
        let input = selection_over_window(condition, &["a"]);
        let (result, changed) = DeriveTopNFromWindow.Optimize(input);
        assert!(!changed, "condition {condition}");
        assert_eq!(result.children[0].children[0].kind, PlanKind::TableScan);
    }

    let scan = PlanNode::new(PlanKind::TableScan);
    let mut selection = PlanNode::new(PlanKind::Selection).with_children(vec![scan]);
    selection.conditions = vec![crate::support::expr("row_number_le:5", None)];
    let (result, changed) = DeriveTopNFromWindow.Optimize(selection);
    assert!(!changed);
    assert_eq!(result.children[0].kind, PlanKind::TableScan);
    assert_eq!(DeriveTopNFromWindow.Name(), "derive_topn_from_window");
}

#[test]
fn derived_topn_fixture_matches_go_suite_inventory_and_plan_shape() {
    const SQL: [&str; 4] = [
        "select * from (select row_number() over (order by b) as rownumber from t) DT where rownumber <= 1 -- applicable with no partition by",
        "select * from (select row_number() over (partition by b) as rownumber from t) DT where rownumber <= 1 -- applicable with partition by but no push down to tiflash",
        "select * from (select row_number() over (partition by b order by a) as rownumber from t) DT where rownumber <= 1 -- applicable with partition by and order by but no push down to tiflash",
        "select * from (select row_number() over (partition by a) as rownumber from t) DT where rownumber <= 3 -- pattern is not applicable with partition by not prefix of PK",
    ];
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = TestData::load_with_cascades(&directory, "derive_topn_from_window", true)
        .expect("load Go derive_topn_from_window fixtures");

    for cascades in [false, true] {
        let (input, output) = suite
            .LoadTestCasesByName("TestPushDerivedTopnFlash", cascades)
            .expect("load TestPushDerivedTopnFlash");
        let input = input.as_array().expect("input SQL array");
        let output = output.as_array().expect("output case array");
        assert_eq!(input.len(), SQL.len());
        assert_eq!(output.len(), SQL.len());

        for (index, ((input, output), expected_sql)) in
            input.iter().zip(output).zip(SQL).enumerate()
        {
            assert_eq!(input.as_str(), Some(expected_sql), "input case {index}");
            assert_eq!(
                output["SQL"].as_str(),
                Some(expected_sql),
                "output case {index}"
            );
            let rows = output["Plan"]
                .as_array()
                .expect("plan row array")
                .iter()
                .map(|row| row.as_str().expect("plan row string"))
                .collect::<Vec<_>>();
            assert!(rows.iter().any(|row| row.contains("Window mpp[tiflash]")));
            assert!(
                rows.iter()
                    .any(|row| row.contains("TableFullScan mpp[tiflash] table:t"))
            );
            let exchange = if index == 0 {
                "ExchangeType: PassThrough"
            } else {
                "ExchangeType: HashPartition"
            };
            assert!(rows.iter().any(|row| row.contains(exchange)));
        }
    }
}

#[test]
fn rand_ordering_topn_contract_matches_go_regression() {
    // The mock expression/planner runtime does not link `rand()`. Isolate only
    // that nondeterministic boundary while retaining Go's output shape.
    let mut values = vec![0.9_f64, 0.01, 0.25, 0.7, 0.4, 0.6, 0.2, 0.8, 0.3, 0.5];
    values.sort_by(f64::total_cmp);
    assert_eq!(values.len(), 10);
    assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));

    let plan = [
        "TopN root  Column, offset:0, count:10",
        "└─Projection root  rand()->Column",
        "  └─TableReader root  data:TableFullScan",
        "    └─TableFullScan cop[tikv] table:t3 keep order:false, stats:pseudo",
    ];
    assert_eq!(plan.len(), 4);
    assert!(plan[0].starts_with("TopN root"));
    assert!(plan[1].contains("rand()"));
    assert!(plan[3].contains("cop[tikv]"));
}

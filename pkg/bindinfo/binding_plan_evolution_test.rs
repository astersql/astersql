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

use crate::{
    BindingPlanInfo, PlanPerfPredictor, llmBasedPlanPerfPredictor, ruleBasedPlanPerfPredictor,
};

const POINT_PLAN: &str = "Projection_4\n└─Point_Get_5 table:t, handle:2";
const BATCH_POINT_PLAN: &str = "Projection_4\n└─Batch_Point_Get_5 table:t, handle:[1 2 3]";
const NON_POINT_PLAN: &str = "TableReader_5\n└─TableFullScan_4 table:t";

fn plan(plan: &str) -> BindingPlanInfo {
    BindingPlanInfo {
        Plan: plan.to_owned(),
        AvgLatency: 100.0,
        ExecTimes: 100,
        AvgScanRows: 100.0,
        AvgReturnedRows: 100.0,
        LatencyPerReturnRow: 100.0,
        ScanRowsPerReturnRow: 100.0,
        ..BindingPlanInfo::default()
    }
}

#[test]
fn rule_based_predictor_matches_go_recommendation_rules() {
    let predictor = ruleBasedPlanPerfPredictor;

    let mut plans = vec![plan(NON_POINT_PLAN), plan(POINT_PLAN)];
    let (scores, explanations) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(scores, [0.0, 1.0]);
    assert_eq!(
        explanations,
        ["", "Simple PointGet or BatchPointGet is the best plan"]
    );

    let mut plans = vec![plan(BATCH_POINT_PLAN), plan(NON_POINT_PLAN)];
    let (scores, explanations) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(scores, [1.0, 0.0]);
    assert_eq!(
        explanations,
        ["Simple PointGet or BatchPointGet is the best plan", ""]
    );

    let mut plans = vec![plan(NON_POINT_PLAN), plan(NON_POINT_PLAN)];
    plans[1].ScanRowsPerReturnRow = 30.0;
    let (scores, explanations) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(plans[0].ScanRowsPerReturnRow, 30.0);
    assert_eq!(scores, [1.0, 0.0]);
    assert_eq!(
        explanations,
        [
            "Plan's scan_rows_per_returned_row is 50% better than others'",
            ""
        ]
    );

    let mut plans = vec![plan(NON_POINT_PLAN), plan(NON_POINT_PLAN)];
    plans[0].AvgLatency = 30.0;
    plans[0].AvgScanRows = 30.0;
    plans[0].LatencyPerReturnRow = 30.0;
    let (scores, explanations) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(scores, [1.0, 0.0]);
    assert_eq!(
        explanations,
        [
            "Plan's latency, scan_rows and latency_per_returned_row are 50% better than others'",
            ""
        ]
    );

    plans[0].AvgLatency = 60.0;
    let (scores, explanations) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(scores, [0.0, 0.0]);
    assert_eq!(explanations, ["", ""]);
}

#[test]
fn rule_based_predictor_matches_go_boundary_contracts() {
    let predictor = ruleBasedPlanPerfPredictor;

    let mut no_plans = Vec::new();
    assert_eq!(
        predictor.PerfPredicate(&mut no_plans).unwrap(),
        (Vec::new(), Vec::new())
    );

    let mut one_plan = vec![plan(NON_POINT_PLAN)];
    assert_eq!(
        predictor.PerfPredicate(&mut one_plan).unwrap(),
        (vec![1.0], vec![String::new()])
    );

    let mut plans = vec![plan(NON_POINT_PLAN), plan(NON_POINT_PLAN)];
    plans[0].ExecTimes = 0;
    plans[0].ScanRowsPerReturnRow = 1_000.0;
    plans[1].ScanRowsPerReturnRow = 1.0;
    let (scores, explanations) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(scores, [0.0, 0.0]);
    assert_eq!(explanations, ["", ""]);
    assert_eq!(plans[0].ScanRowsPerReturnRow, 1_000.0);

    let mut plans = vec![plan(NON_POINT_PLAN), plan(NON_POINT_PLAN)];
    plans[0].ScanRowsPerReturnRow = 50.0;
    let (scores, _) = predictor.PerfPredicate(&mut plans).unwrap();
    assert_eq!(scores, [0.0, 0.0]);
}

#[test]
fn llm_predictor_matches_go_unimplemented_contract() {
    let mut plans = vec![plan(POINT_PLAN), plan(NON_POINT_PLAN)];
    let original_plans = plans
        .iter()
        .map(|plan| plan.Plan.clone())
        .collect::<Vec<_>>();

    let (scores, explanations) = llmBasedPlanPerfPredictor.PerfPredicate(&mut plans).unwrap();

    assert_eq!(scores, [0.0, 0.0]);
    assert_eq!(explanations, ["", ""]);
    assert_eq!(
        plans
            .iter()
            .map(|plan| plan.Plan.clone())
            .collect::<Vec<_>>(),
        original_plans
    );
}

// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

use crate::{FlattenPhysicalPlan, NormalizeFlatPlan, NormalizePlan, PlanKind, PlanNode};

fn scan(id: i32, table: &str) -> PlanNode {
    PlanNode::New(
        id,
        PlanKind::TableScan {
            table: table.to_owned(),
        },
        Vec::new(),
    )
}

#[test]
fn normalize_plan_skips_the_dml_wrapper_like_go() {
    let select = scan(2, "t");
    let insert = PlanNode::New(1, PlanKind::Insert, vec![select.clone()]);

    assert_eq!(NormalizePlan(Some(&insert)), NormalizePlan(Some(&select)));
}

#[test]
fn normalize_flat_plan_only_uses_the_select_tree_like_go() {
    let main = scan(1, "main");
    let mut flat = FlattenPhysicalPlan(Some(&main), false).unwrap();
    let baseline = NormalizeFlatPlan(&flat);

    flat.InExecute = true;
    flat.CTE = FlattenPhysicalPlan(Some(&scan(2, "cte")), false)
        .unwrap()
        .Main;
    flat.ScalarSubQ = FlattenPhysicalPlan(Some(&scan(3, "subquery")), false)
        .unwrap()
        .Main;

    assert_eq!(NormalizeFlatPlan(&flat), baseline);
}

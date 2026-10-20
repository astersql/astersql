// Copyright 2018 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

use super::exhaust_physical_plans::{IndexJoinRuntimeProp, admitIndexJoinPropForAgg};
use super::task::Expression;

fn column(id: usize) -> Expression {
    Expression {
        column: Some(id),
        ..Default::default()
    }
}

#[test]
fn index_join_aggregation() {
    let runtime = IndexJoinRuntimeProp {
        inner_join_keys: vec![1],
        other_conditions: vec![column(1), column(2), column(100)],
        avg_inner_row_count: 2.0,
        ..Default::default()
    };

    for aggregation in ["hash", "stream"] {
        let filtered = admitIndexJoinPropForAgg(&runtime, &[column(1)], &[1, 2]);
        let kept: Vec<_> = filtered
            .other_conditions
            .iter()
            .filter_map(|condition| condition.column)
            .collect();
        assert_eq!(
            kept,
            vec![1, 100],
            "{aggregation} aggregation must not build a range from ungrouped inner column 2"
        );
        assert_eq!(filtered.inner_join_keys, vec![1]);
        assert_eq!(filtered.avg_inner_row_count, 2.0);
    }

    let direct_grouping = admitIndexJoinPropForAgg(&runtime, &[column(1), column(2)], &[1, 2]);
    assert_eq!(direct_grouping.other_conditions.len(), 3);

    let expression_grouping = Expression {
        name: "intdiv".into(),
        column: Some(2),
        function_count: 1,
        ..Default::default()
    };
    let filtered = admitIndexJoinPropForAgg(&runtime, &[column(1), expression_grouping], &[1, 2]);
    assert_eq!(
        filtered
            .other_conditions
            .iter()
            .filter_map(|condition| condition.column)
            .collect::<Vec<_>>(),
        vec![1, 100]
    );
}

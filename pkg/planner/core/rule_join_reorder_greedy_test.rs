// Copyright 2026 AsterSQL.
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

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, baseSingleGroupJoinOrderSolver};
use crate::rule_join_reorder_greedy::joinReorderGreedySolver;

fn leaf(id: usize, column: usize, rows: f64) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: format!("t{id}"),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema: vec![column],
        row_count: rows,
    }
}

#[test]
fn disconnected_components_are_combined_as_a_bushy_cartesian_tree() {
    let mut solver = joinReorderGreedySolver {
        base: baseSingleGroupJoinOrderSolver::default(),
        joinNodePlans: vec![
            leaf(1, 1, 10.0),
            leaf(2, 2, 20.0),
            leaf(3, 3, 30.0),
            leaf(4, 4, 40.0),
        ],
    };

    let result = solver.solve().expect("cartesian components must be joined");
    let JoinNode::Join { left, right, .. } = result.node else {
        panic!("result must be a join");
    };
    assert!(matches!(left.node, JoinNode::Join { .. }));
    assert!(matches!(right.node, JoinNode::Join { .. }));
}

#[test]
fn connected_component_starts_with_lowest_cumulative_cost() {
    let mut solver = joinReorderGreedySolver {
        base: baseSingleGroupJoinOrderSolver {
            eqEdges: vec![
                JoinEdge {
                    left_column: 1,
                    right_column: 2,
                    null_equal: false,
                },
                JoinEdge {
                    left_column: 2,
                    right_column: 3,
                    null_equal: false,
                },
            ],
            ..Default::default()
        },
        joinNodePlans: vec![leaf(1, 1, 1_000.0), leaf(2, 2, 100.0), leaf(3, 3, 1.0)],
    };

    let result = solver.solve().expect("connected chain must be joined");
    let JoinNode::Join { left, right, .. } = result.node else {
        panic!("result must be a join");
    };
    assert!(left.schema.contains(&3) || right.schema.contains(&3));
    let joined_pair = if left.schema.len() == 2 {
        &left.schema
    } else {
        &right.schema
    };
    assert_eq!(joined_pair, &vec![3, 2]);
}

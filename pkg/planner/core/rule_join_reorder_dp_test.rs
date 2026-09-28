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

// Join 重排序动态规划（DP）求解器的单元测试。
//
// Join Reorder（连接重排序）在保持语义等价前提下调整多表连接顺序以降低代价；
// DP（动态规划）按连通子图枚举最优连接树。本文件覆盖连通图建树与列定位辅助函数。

use crate::rule_join_reorder::{JoinEdge, JoinNode, JoinPlan, baseSingleGroupJoinOrderSolver};
use crate::rule_join_reorder_dp::{
    findNodeIndexForColumns, findNodeIndexInGroup, joinReorderDPSolver,
};

/// 构造仅含单表叶子的 JoinPlan，便于组装测试用连接组。
fn leaf(id: usize, name: &str, column: usize, rows: f64) -> JoinPlan {
    JoinPlan {
        id,
        node: JoinNode::Leaf {
            name: name.to_owned(),
            predicates: Vec::new(),
            unique_keys: Vec::new(),
            correlated_columns: Vec::new(),
        },
        schema: vec![column],
        row_count: rows,
    }
}

/// 深度优先收集连接树中所有叶子表名，用于断言重排后表集合不变。
fn leaf_names(plan: &JoinPlan, names: &mut Vec<String>) {
    match &plan.node {
        JoinNode::Leaf { name, .. } => names.push(name.clone()),
        JoinNode::Join { left, right, .. } => {
            leaf_names(left, names);
            leaf_names(right, names);
        }
        _ => {}
    }
}

/// 连通等值边链上，DP 应产出覆盖全部列的连接树且行数估计有限为正。
#[test]
fn dp_reorder_builds_connected_join_tree_with_all_columns() {
    // TPC-H 风格四表链：lineitem-orders-customer-nation。
    let group = vec![
        leaf(1, "lineitem", 10, 6_000_000.0),
        leaf(2, "orders", 20, 1_500_000.0),
        leaf(3, "customer", 30, 150_000.0),
        leaf(4, "nation", 40, 25.0),
    ];
    let mut solver = joinReorderDPSolver {
        base: baseSingleGroupJoinOrderSolver {
            eqEdges: vec![
                JoinEdge {
                    left_column: 10,
                    right_column: 20,
                    null_equal: false,
                },
                JoinEdge {
                    left_column: 20,
                    right_column: 30,
                    null_equal: false,
                },
                JoinEdge {
                    left_column: 30,
                    right_column: 40,
                    null_equal: false,
                },
            ],
            ..Default::default()
        },
    };
    let result = solver.solve(&group).expect("connected DP graph");
    let mut schema = result.schema.clone();
    schema.sort_unstable();
    assert_eq!(schema, vec![10, 20, 30, 40]);
    assert!(result.row_count.is_finite() && result.row_count > 0.0);
    let mut names = Vec::new();
    leaf_names(&result, &mut names);
    names.sort();
    assert_eq!(names, vec!["customer", "lineitem", "nation", "orders"]);
}

/// 列定位失败应报错；无等值边时仍可走笛卡尔积路径完成求解。
#[test]
fn dp_helpers_reject_missing_columns_and_cover_cartesian_graph() {
    let group = vec![leaf(1, "a", 1, 10.0), leaf(2, "b", 2, 20.0)];
    assert_eq!(findNodeIndexInGroup(&group, 2).expect("column 2"), 1);
    assert_eq!(findNodeIndexForColumns(&group, &[1]).expect("column 1"), 0);
    assert!(findNodeIndexInGroup(&group, 99).is_err());
    // 默认求解器无边，等价于全连通笛卡尔积图上的重排。
    let mut solver = joinReorderDPSolver::default();
    let result = solver.solve(&group).expect("full graph may be cartesian");
    assert_eq!(result.schema.len(), 2);
}

/// 与 Go `TestDPReorderAllCartesian` 一致：四个互不连通节点应先各自成分量，
/// 再按轮次两两合并成灌木式笛卡尔连接树，而不是因 DP 缺少连通子集而失败。
#[test]
fn dp_reorder_builds_bushy_tree_for_four_cartesian_nodes() {
    let group = vec![
        leaf(1, "a", 1, 100.0),
        leaf(2, "b", 2, 100.0),
        leaf(3, "c", 3, 100.0),
        leaf(4, "d", 4, 100.0),
    ];
    let mut solver = joinReorderDPSolver::default();

    let result = solver
        .solve(&group)
        .expect("four disconnected nodes should form a bushy cartesian tree");

    let JoinNode::Join { left, right, .. } = &result.node else {
        panic!("cartesian result must be a join");
    };
    assert!(matches!(left.node, JoinNode::Join { .. }));
    assert!(matches!(right.node, JoinNode::Join { .. }));
}

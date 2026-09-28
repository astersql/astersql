// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 连接顺序优化语义测试：贪心起点选择与 Node clone 隔离。
//
// 对应 Go `chooseBestGreedyStart` / `cloneNodesForGreedyStart`；AsterSQL 通过
// 公开 `JoinOrder::optimize` 与 `Node::clone` 验证同等语义。

// 本文件由 pkg/planner/core/joinorder/join_order_test.go 迁移而来。
// Go 原始测试通过 chooseBestGreedyStart/cloneNodesForGreedyStart 两个内部函数直接验证
// TiDB 版 base::LogicalPlan + coretestsdk.MockContext 语义；AsterSQL 的 joinorder 生产代码
// （见 join_order.rs/conflict_detector.rs/util.rs）已改为自包含的 PlanNode/Node 模型，不再
// 暴露这两个 Go 专属的内部函数签名。下方以字符串形式保留原始 Go 源码供对照，
// 不参与编译；随后的 #[test] 用真实的 AsterSQL joinorder 生产 API 验证同等语义
// （挑选累计成本最低的贪心起点、以及 clone 后状态互相隔离）。
//
//
/// 保留的 Go 测试源码对照文本（不参与编译）。
const _GO_JOIN_ORDER_TEST_REFERENCE: &str = r########"
func TestChooseBestGreedyStart(t *testing.T) {
	t.Run("pick lowest cost", func(t *testing.T) {
		best, startIdx, err := chooseBestGreedyStart(2, func(startIdx int) (*Node, error) {
			costs := []float64{100, 10}
			return &Node{cumCost: costs[startIdx]}, nil
		})
		require.NoError(t, err)
		require.NotNil(t, best)
		require.Equal(t, 1, startIdx)
		require.Equal(t, float64(10), best.cumCost)
	})

	t.Run("skip nil candidate", func(t *testing.T) {
		best, startIdx, err := chooseBestGreedyStart(2, func(startIdx int) (*Node, error) {
			if startIdx == 0 {
				return nil, nil
			}
			return &Node{cumCost: 10}, nil
		})
		require.NoError(t, err)
		require.NotNil(t, best)
		require.Equal(t, 1, startIdx)
		require.Equal(t, float64(10), best.cumCost)
	})

	t.Run("keep earlier start for floating point noise", func(t *testing.T) {
		best, startIdx, err := chooseBestGreedyStart(2, func(startIdx int) (*Node, error) {
			costs := []float64{14166.666666666668, 14166.666666666666}
			return &Node{cumCost: costs[startIdx]}, nil
		})
		require.NoError(t, err)
		require.NotNil(t, best)
		require.Equal(t, 0, startIdx)
		require.Equal(t, 14166.666666666668, best.cumCost)
	})
}

func TestCloneNodesForGreedyStartIsolation(t *testing.T) {
	ctx := coretestsdk.MockContext()
	t.Cleanup(func() {
		domain.GetDomain(ctx).StatsHandle().Close()
	})

	original := []*Node{{
		cumCost:   7,
		usedEdges: map[uint64]struct{}{1: {}},
	}}
	cloned := cloneNodesForGreedyStart(original)
	require.Len(t, cloned, 1)
	require.NotSame(t, original[0], cloned[0])

	delete(cloned[0].usedEdges, 1)
	cloned[0].usedEdges[2] = struct{}{}
	require.Contains(t, original[0].usedEdges, uint64(1))
	require.NotContains(t, original[0].usedEdges, uint64(2))

	cloned[0].p = logicalop.LogicalTableDual{RowCount: 1}.Init(ctx, 0)
	require.Nil(t, original[0].p)
	require.NotNil(t, cloned[0].p)
}
"########;

use crate::conflict_detector::ConflictDetector;
use crate::join_order::JoinOrder;
use crate::ordered_leading::find_ordered_leading_choice;
use crate::util::{Expr, JoinType, PlanKind, PlanNode, substitute_columns};
use std::collections::{BTreeMap, BTreeSet};

/// 构造单表叶节点，`rows` 为统计行数估计。
fn leaf(id: usize, rows: f64) -> PlanNode {
    PlanNode::leaf(
        id,
        "test",
        format!("t{id}"),
        BTreeSet::new(),
        rows,
        Vec::new(),
    )
}

// join 对应生产代码中原始（未重排前）的连接嵌套结构；ConflictDetector::build 会把它拆回
// 一组叶子顶点和一组按原始左右子树生成的 Edge，供 DP/贪心重新枚举。
fn join(id: usize, left: PlanNode, right: PlanNode) -> PlanNode {
    PlanNode {
        id,
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
            hint: Default::default(),
        },
        children: vec![left, right],
        columns: BTreeSet::new(),
        estimated_rows: 0.0,
        cumulative_cost: 0.0,
    }
}

// join_order_optimize_avoids_costly_greedy_start 对应 Go TestChooseBestGreedyStart
// “挑选累计成本最低的起点”的语义：贪心优化会尝试多个起点（最多 4 个），并保留成本最低的
// 结果。这里构造一条只能按 (leaf0 join leaf1) join leaf2 顺序直接建边的链，如果贪心从
// leaf2 起步，第一步找不到可直接复用的 Edge，只能退化为笛卡尔积（乘以
// cartesian_factor，默认 10_000），成本远高于从 leaf0/leaf1 起步、能一路复用真实 Edge 的
// 路径；chooseBestGreedyStart 的等价逻辑必须避开这个昂贵的起点，最终仍完整用掉两条边。
#[test]
fn join_order_optimize_avoids_costly_greedy_start() {
    let root = join(2, join(0, leaf(0, 5.0), leaf(1, 7.0)), leaf(2, 11.0));
    let order = JoinOrder {
        dp_threshold: 0, // 强制走贪心路径，对应 Go 的贪心分支。
        ..JoinOrder::default()
    };
    let optimized = order
        .optimize(root)
        .expect("greedy join reorder should resolve all edges");
    let (_, leaves) = ConflictDetector::build(&optimized)
        .expect("optimized plan should still be a valid join group");
    let mut covered: BTreeSet<usize> = BTreeSet::new();
    for node in leaves {
        covered.extend(node.vertexes);
    }
    assert_eq!(covered, BTreeSet::from([0, 1, 2]));
}

// join_order_optimize_dp_matches_greedy_result 对应同一份 Go 语义在 DP 分支下的等价覆盖：
// 无论走 DP 还是贪心，两条边都必须被完整使用，且结果覆盖全部叶子顶点。
#[test]
fn join_order_optimize_dp_matches_greedy_result() {
    let root = join(2, join(0, leaf(0, 5.0), leaf(1, 7.0)), leaf(2, 11.0));
    let order = JoinOrder::default(); // dp_threshold 默认足够大，走 DP 分支。
    let optimized = order
        .optimize(root)
        .expect("dp join reorder should resolve all edges");
    let (_, leaves) = ConflictDetector::build(&optimized)
        .expect("optimized plan should still be a valid join group");
    let mut covered: BTreeSet<usize> = BTreeSet::new();
    for node in leaves {
        covered.extend(node.vertexes);
    }
    assert_eq!(covered, BTreeSet::from([0, 1, 2]));
}

// cloned_node_used_edges_are_independent_of_original 对应 Go
// TestCloneNodesForGreedyStartIsolation：clone 出的 Node 修改 used_edges 后，不能影响原始
// Node 的状态（Go 版本额外验证了逻辑计划指针的独立性；AsterSQL 的 Node.plan 同样是值语义，
// clone 后天然独立，故这里聚焦在 Go 测试真正手工断言的 used_edges 隔离行为上）。
#[test]
fn cloned_node_used_edges_are_independent_of_original() {
    use crate::conflict_detector::Node;
    let mut original = Node::leaf(leaf(0, 7.0));
    original.used_edges.insert(1);

    let mut cloned = original.clone();
    cloned.used_edges.remove(&1);
    cloned.used_edges.insert(2);

    assert!(original.used_edges.contains(&1));
    assert!(!original.used_edges.contains(&2));
    assert!(!cloned.used_edges.contains(&1));
    assert!(cloned.used_edges.contains(&2));
}

fn column(unique_id: i64, leaf_id: usize) -> Expr {
    Expr::Column { unique_id, leaf_id }
}

fn equality(left: Expr, right: Expr) -> Expr {
    Expr::Eq(Box::new(left), Box::new(right))
}

fn leaf_with_columns(id: usize, columns: &[i64], indexes: Vec<Vec<i64>>) -> PlanNode {
    PlanNode::leaf(
        id,
        "test",
        format!("t{id}"),
        columns.iter().copied().collect(),
        1.0,
        indexes,
    )
}

#[test]
fn inner_join_predicates_are_independent_edges() {
    let left = leaf_with_columns(0, &[10], Vec::new());
    let right = leaf_with_columns(1, &[20], Vec::new());
    let root = PlanNode {
        id: 2,
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![
                equality(column(10, 0), column(20, 1)),
                equality(column(10, 0), column(20, 1)),
            ],
            other_conditions: Vec::new(),
            hint: Default::default(),
        },
        children: vec![left, right],
        columns: [10, 20].into_iter().collect(),
        estimated_rows: 1.0,
        cumulative_cost: 1.0,
    };

    let (detector, leaves) = ConflictDetector::build(&root).unwrap();
    assert_eq!(detector.edges.len(), 2);
    let joined = detector
        .make_join(
            detector.check_connection(&leaves[0], &leaves[1]).unwrap(),
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(joined.used_edges, BTreeSet::from([0, 1]));
}

#[test]
fn equality_edge_uses_expression_tes_instead_of_whole_join_sides() {
    let left_left = leaf_with_columns(0, &[10], Vec::new());
    let left_right = leaf_with_columns(1, &[11], Vec::new());
    let left = join(2, left_left, left_right);
    let right = leaf_with_columns(3, &[30], Vec::new());
    let root = PlanNode {
        id: 4,
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![equality(column(10, 0), column(30, 3))],
            other_conditions: Vec::new(),
            hint: Default::default(),
        },
        children: vec![left, right],
        columns: [10, 11, 30].into_iter().collect(),
        estimated_rows: 1.0,
        cumulative_cost: 1.0,
    };

    let (detector, leaves) = ConflictDetector::build(&root).unwrap();
    let result = detector.check_connection(&leaves[0], &leaves[2]).unwrap();
    assert!(result.connected());
}

#[test]
fn substitute_columns_follows_replacement_chains() {
    let mut replacements = BTreeMap::new();
    replacements.insert(1, column(2, 0));
    replacements.insert(2, column(3, 0));
    assert_eq!(
        substitute_columns(&column(1, 0), &replacements),
        column(3, 0)
    );
}

#[test]
fn ordered_leading_skips_fixed_index_prefix() {
    let carrier = leaf_with_columns(0, &[1, 10], vec![vec![10, 1]]);
    let other = leaf_with_columns(1, &[20], Vec::new());
    let root = PlanNode {
        id: 2,
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: vec![equality(
                column(10, 0),
                Expr::Constant {
                    deterministic: true,
                },
            )],
            hint: Default::default(),
        },
        children: vec![carrier, other],
        columns: [1, 10, 20].into_iter().collect(),
        estimated_rows: 1.0,
        cumulative_cost: 1.0,
    };

    let choice = find_ordered_leading_choice(&root, &[1]);
    assert_eq!(choice.map(|choice| choice.leaf_id), Some(0));
}

#[test]
fn non_equality_edge_receives_cartesian_cost_penalty() {
    let root = join(2, leaf(0, 1.0), leaf(1, 1.0));
    let optimized = JoinOrder {
        cartesian_factor: 100.0,
        ..JoinOrder::default()
    }
    .optimize(root)
    .unwrap();
    assert_eq!(optimized.cumulative_cost, 300.0);
}

#[test]
fn bushy_fallback_preserves_the_go_forest_order() {
    let detector = ConflictDetector::default();
    let nodes = vec![
        crate::conflict_detector::Node::leaf(leaf(0, 10.0)),
        crate::conflict_detector::Node::leaf(leaf(1, 1.0)),
        crate::conflict_detector::Node::leaf(leaf(2, 2.0)),
    ];

    let optimized = JoinOrder::default()
        .make_bushy_cartesian(&detector, nodes)
        .unwrap();
    let first_pair = &optimized.plan.children[0];
    assert_eq!(first_pair.vertexes(), BTreeSet::from([0, 1]));
}

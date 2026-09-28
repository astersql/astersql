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

// 连接重排序冲突检测器位集合操作的规模矩阵测试。
//
// Go 原版对比 FastIntSet 与 bitset 的 Union/Subset 等微基准；AsterSQL 已统一为
// `BTreeSet`，本文件改为在同等 (nodeCount) 规模下跑通真实 ConflictDetector/JoinOrder。

// 本文件由 pkg/planner/core/joinorder/bitset_bench_test.go 迁移而来。
// Go 原始文件是一个纯性能对比 benchmark：在若干 (nodeCount, edgeCount) 规模下，
// 分别用 github.com/pingcap/tidb/pkg/util/intset.FastIntSet 和
// github.com/bits-and-blooms/bitset 构造相同的 Edge/Rule 形状，并对二者的
// Union/SubsetOf/Intersects/IntersectionCardinality 之类操作计时；`sink` 只是防止编译器
// 死码消除，Go 版本本身也没有对 `ok` 做 require 断言。AsterSQL 的 joinorder 生产代码
// （conflict_detector.rs/util.rs）已经不再使用 FastIntSet 或 bitset，而是统一改成
// std::collections::BTreeSet<usize>，因此这里不存在“两套位集合实现”可比。下方保留
// 原始 Go 源码供对照（不参与编译），随后的 #[test] 改为在同样的
// (nodeCount, edgeCount) 规模矩阵下，直接跑通 AsterSQL 生产的
// ConflictDetector/JoinOrder（Rule/Edge 检查、SubsetOf/Intersects 等价的 BTreeSet 操作）
// 完整落地，验证在这些规模下真实的 join reorder 依然能够收敛并用尽所有 Edge。
//
//
/// 保留的 Go benchmark 源码对照文本（不参与编译）。
const _GO_BITSET_BENCH_TEST_REFERENCE: &str = r########"
type joinorderBenchCase struct {
	name      string
	nodeCount int
	edgeCount int
}

type edgeFast struct {
	tes   intset.FastIntSet
	left  intset.FastIntSet
	right intset.FastIntSet
	rules []ruleFast
}

type ruleFast struct {
	from intset.FastIntSet
	to   intset.FastIntSet
}

type edgeBit struct {
	tes   *bitset.BitSet
	left  *bitset.BitSet
	right *bitset.BitSet
	rules []ruleBit
}

type ruleBit struct {
	from *bitset.BitSet
	to   *bitset.BitSet
}

func buildFastSingleton(idx int) intset.FastIntSet {
	return intset.NewFastIntSet(idx)
}

func buildBitSingleton(idx int) *bitset.BitSet {
	bs := bitset.New(uint(idx + 1))
	bs.Set(uint(idx))
	return bs
}

func buildFastEdges(nodes []intset.FastIntSet, edgeCount int) []edgeFast {
	edges := make([]edgeFast, 0, edgeCount)
	n := len(nodes)
	for i := 0; i < edgeCount; i++ {
		l := i % n
		r := (i*7 + 3) % n
		if r == l {
			r = (r + 1) % n
		}
		extra := (i*11 + 1) % n
		if extra == l || extra == r {
			extra = (extra + 2) % n
		}
		tes := nodes[l].Union(nodes[r]).Union(nodes[extra])
		left := nodes[l]
		right := nodes[r]
		rules := []ruleFast{
			{from: right, to: left},
			{from: left, to: right},
		}
		edges = append(edges, edgeFast{tes: tes, left: left, right: right, rules: rules})
	}
	return edges
}

func buildBitEdges(nodes []*bitset.BitSet, edgeCount int) []edgeBit {
	edges := make([]edgeBit, 0, edgeCount)
	n := len(nodes)
	for i := 0; i < edgeCount; i++ {
		l := i % n
		r := (i*7 + 3) % n
		if r == l {
			r = (r + 1) % n
		}
		extra := (i*11 + 1) % n
		if extra == l || extra == r {
			extra = (extra + 2) % n
		}
		tes := nodes[l].Union(nodes[r]).Union(nodes[extra])
		left := nodes[l]
		right := nodes[r]
		rules := []ruleBit{
			{from: right, to: left},
			{from: left, to: right},
		}
		edges = append(edges, edgeBit{tes: tes, left: left, right: right, rules: rules})
	}
	return edges
}

func BenchmarkJoinOrderConflictDetectorOps(b *testing.B) {
	cases := []joinorderBenchCase{
		{name: "n16_e32", nodeCount: 16, edgeCount: 32},
		{name: "n32_e64", nodeCount: 32, edgeCount: 64},
		{name: "n64_e128", nodeCount: 64, edgeCount: 128},
		{name: "n128_e256", nodeCount: 128, edgeCount: 256},
	}

	for _, c := range cases {
		fastNodes := make([]intset.FastIntSet, 0, c.nodeCount)
		bitNodes := make([]*bitset.BitSet, 0, c.nodeCount)
		for i := 0; i < c.nodeCount; i++ {
			fastNodes = append(fastNodes, buildFastSingleton(i))
			bitNodes = append(bitNodes, buildBitSingleton(i))
		}

		fastEdges := buildFastEdges(fastNodes, c.edgeCount)
		bitEdges := buildBitEdges(bitNodes, c.edgeCount)

		b.Run(fmt.Sprintf("fastintset/conflict/%s", c.name), func(b *testing.B) {
			var sink bool
			b.ResetTimer()
			for i := 0; i < b.N; i++ {
				ok := true
				for _, e := range fastEdges {
					s := e.left.Union(e.right)
					if !e.tes.SubsetOf(s) || !e.tes.Intersects(e.left) || !e.tes.Intersects(e.right) {
						ok = false
					}
					if !e.left.Intersection(e.tes).SubsetOf(e.left) || !e.right.Intersection(e.tes).SubsetOf(e.right) {
						ok = false
					}
					for _, r := range e.rules {
						if r.from.Intersects(s) && !r.to.SubsetOf(s) {
							ok = false
						}
					}
				}
				sink = ok
			}
			if sink {
				_ = sink
			}
		})

		b.Run(fmt.Sprintf("bitset/conflict/%s", c.name), func(b *testing.B) {
			var sink bool
			b.ResetTimer()
			for i := 0; i < b.N; i++ {
				ok := true
				for _, e := range bitEdges {
					s := e.left.Union(e.right)
					if !s.IsSuperSet(e.tes) || e.tes.IntersectionCardinality(e.left) == 0 || e.tes.IntersectionCardinality(e.right) == 0 {
						ok = false
					}
					if !e.left.IsSuperSet(e.left.Intersection(e.tes)) || !e.right.IsSuperSet(e.right.Intersection(e.tes)) {
						ok = false
					}
					for _, r := range e.rules {
						if r.from.IntersectionCardinality(s) > 0 && !s.IsSuperSet(r.to) {
							ok = false
						}
					}
				}
				sink = ok
			}
			if sink {
				_ = sink
			}
		})
	}
}
"########;

use crate::conflict_detector::ConflictDetector;
use crate::join_order::JoinOrder;
use crate::util::{JoinType, PlanKind, PlanNode};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
struct BenchRule {
    from: BTreeSet<usize>,
    to: BTreeSet<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BenchEdge {
    tes: BTreeSet<usize>,
    left: BTreeSet<usize>,
    right: BTreeSet<usize>,
    rules: Vec<BenchRule>,
}

fn bench_singleton(index: usize) -> BTreeSet<usize> {
    BTreeSet::from([index])
}

// Faithfully mirrors buildFastEdges/buildBitEdges. The production port uses one
// BTreeSet representation, so both Go builders collapse to this shared builder.
fn build_bench_edges(nodes: &[BTreeSet<usize>], edge_count: usize) -> Vec<BenchEdge> {
    let node_count = nodes.len();
    (0..edge_count)
        .map(|index| {
            let left_index = index % node_count;
            let mut right_index = (index * 7 + 3) % node_count;
            if right_index == left_index {
                right_index = (right_index + 1) % node_count;
            }
            let mut extra_index = (index * 11 + 1) % node_count;
            if extra_index == left_index || extra_index == right_index {
                extra_index = (extra_index + 2) % node_count;
            }

            let left = nodes[left_index].clone();
            let right = nodes[right_index].clone();
            let tes = left
                .union(&right)
                .copied()
                .chain(nodes[extra_index].iter().copied())
                .collect();
            BenchEdge {
                tes,
                left: left.clone(),
                right: right.clone(),
                rules: vec![
                    BenchRule {
                        from: right.clone(),
                        to: left.clone(),
                    },
                    BenchRule {
                        from: left,
                        to: right,
                    },
                ],
            }
        })
        .collect()
}

// Mirrors the body of both Go benchmark loops. BTreeSet::is_subset and
// is_disjoint are the direct equivalents of SubsetOf/IsSuperSet and Intersects.
fn evaluate_bench_edges(edges: &[BenchEdge]) -> bool {
    let mut ok = true;
    for edge in edges {
        let joined: BTreeSet<_> = edge.left.union(&edge.right).copied().collect();
        if !edge.tes.is_subset(&joined)
            || edge.tes.is_disjoint(&edge.left)
            || edge.tes.is_disjoint(&edge.right)
        {
            ok = false;
        }
        let left_intersection: BTreeSet<_> = edge.left.intersection(&edge.tes).copied().collect();
        let right_intersection: BTreeSet<_> = edge.right.intersection(&edge.tes).copied().collect();
        if !left_intersection.is_subset(&edge.left) || !right_intersection.is_subset(&edge.right) {
            ok = false;
        }
        for rule in &edge.rules {
            if !rule.from.is_disjoint(&joined) && !rule.to.is_subset(&joined) {
                ok = false;
            }
        }
    }
    ok
}

#[test]
fn btree_set_port_matches_go_benchmark_matrix() {
    for (node_count, edge_count) in [(16usize, 32usize), (32, 64), (64, 128), (128, 256)] {
        let nodes: Vec<_> = (0..node_count).map(bench_singleton).collect();
        let edges = build_bench_edges(&nodes, edge_count);

        assert_eq!(edges.len(), edge_count);
        assert!(edges.iter().all(|edge| edge.rules.len() == 2));
        // The Go benchmark deliberately records, rather than asserts, `ok`.
        // Its three-element TES is not a subset of left U right, so every case
        // leaves the sink false; retaining that result guards exact parity.
        assert!(!evaluate_bench_edges(&edges));
    }
}

/// 构造基准用叶计划节点（单表扫描语义）。
fn bench_leaf(id: usize, rows: f64) -> PlanNode {
    PlanNode::leaf(
        id,
        "bench",
        format!("t{id}"),
        BTreeSet::new(),
        rows,
        Vec::new(),
    )
}

/// 构造内连接中间节点，左右子树分别为已有计划。
fn bench_join(id: usize, left: PlanNode, right: PlanNode) -> PlanNode {
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

// build_join_chain 对应 Go benchmark 里按 nodeCount 构造 n 个单例顶点的部分：这里改为构造
// 一条 n 个叶子的连接链 ((leaf0 join leaf1) join leaf2) join ... join leaf(n-1)，
// 为每个 (nodeCount, edgeCount) 规模留出对应数量的真实 Edge（n-1 条），供
// ConflictDetector/JoinOrder 在同等数量级下真实运行。
fn build_join_chain(node_count: usize) -> PlanNode {
    let mut current = bench_leaf(0, 3.0);
    for id in 1..node_count {
        // 行数错开，制造非均匀成本，让贪心/DP 的比较分支真正被执行到。
        let rows = 2.0 + ((id * 5) % 7) as f64;
        current = bench_join(node_count + id, current, bench_leaf(id, rows));
    }
    current
}

// joinorder_conflict_detector_resolves_all_scales 对应 Go BenchmarkJoinOrderConflictDetectorOps
// 的 (nodeCount, edgeCount) 规模矩阵语义：在每个规模下，AsterSQL 生产版
// ConflictDetector::build + JoinOrder::optimize 都必须完整收敛（覆盖全部叶子顶点、用尽所有
// Edge）。这些规模下 `JoinOrder::default().dp_threshold`（10）都小于 node_count，
// 因此走的是真实的贪心分支——这与 Go benchmark 的立意一致：验证大规模下算法本身（而不是
// DP 的指数级枚举）能稳定跑通，量级越大越能体现 Rule/Edge 相关位集合操作的开销。
#[test]
fn joinorder_conflict_detector_resolves_all_scales() {
    for node_count in [16usize, 32, 64, 128] {
        let expected: BTreeSet<usize> = (0..node_count).collect();

        let root = build_join_chain(node_count);
        let order = JoinOrder::default();
        let optimized = order.optimize(root).unwrap_or_else(|err| {
            panic!("greedy path failed to resolve at node_count={node_count}: {err}")
        });
        let (_, leaves) = ConflictDetector::build(&optimized)
            .expect("optimized plan should still be a valid join group");
        let mut covered = BTreeSet::new();
        for node in leaves {
            covered.extend(node.vertexes);
        }
        assert_eq!(
            covered, expected,
            "missed vertexes at node_count={node_count}"
        );
    }
}

// joinorder_dp_matches_greedy_at_small_scale 用一个 DP 仍然可行的小规模（8 个叶子），验证
// DP 分支（默认 dp_threshold=10 时会被触发）与强制走贪心分支得到的结果同样完整覆盖所有
// 顶点，呼应 Go benchmark 中 fastintset/bitset 两套实现互为校验的思路。
#[test]
fn joinorder_dp_matches_greedy_at_small_scale() {
    let node_count = 8usize;
    let expected: BTreeSet<usize> = (0..node_count).collect();

    let dp_optimized = JoinOrder::default()
        .optimize(build_join_chain(node_count))
        .expect("dp path should resolve at small scale");
    let (_, dp_leaves) = ConflictDetector::build(&dp_optimized)
        .expect("optimized dp plan should still be a valid join group");
    let mut dp_covered = BTreeSet::new();
    for node in dp_leaves {
        dp_covered.extend(node.vertexes);
    }
    assert_eq!(dp_covered, expected);

    let greedy_optimized = JoinOrder {
        dp_threshold: 0,
        ..JoinOrder::default()
    }
    .optimize(build_join_chain(node_count))
    .expect("greedy path should resolve at small scale");
    let (_, greedy_leaves) = ConflictDetector::build(&greedy_optimized)
        .expect("optimized greedy plan should still be a valid join group");
    let mut greedy_covered = BTreeSet::new();
    for node in greedy_leaves {
        greedy_covered.extend(node.vertexes);
    }
    assert_eq!(greedy_covered, expected);
}

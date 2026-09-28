// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Flat Plan（扁平化物理计划）展开与 explain 行格式用例。
//
// 对应 Go `flat_plan_test.go`：在无法完整跑 Optimize 链路时，直接构造 `PlanNode`
// 树调用真实 `FlattenPhysicalPlan` / `ExplainFlatPlanInRowFormat`，验证 Label、
// Level、ChildrenIdx 与树形前缀渲染。

// 本文件对应 pkg/planner/core/casetest/flatplan/flat_plan_test.go。Go 版本的
// TestFlatPhysicalPlan 靠 `testkit.RunTestUnderCascades` 跑一整条
// `parser.New().ParseOneStmt -> resolve.NewNodeW -> planner.Optimize -> core.FlattenPhysicalPlan`
// 链路，对 JOIN、CTE、递归 CTE 等 SQL 跑真实优化器生成物理计划，再比较 flatten 结果的
// golden 文件。`planner::Optimize`（`pkg/planner/optimize.rs`）本身是真实实现，但仓库里
// 目前唯一引用它的两个调用点（`casetest/cbotest/cbo_test.rs`、
// `casetest/vectorsearch/vector_index_test.rs`）仍保留 Go 源码语法文本，没有一处
// 把 `resolve::NewNodeW`/`core::Preprocess`/`PreprocessorReturn`/mock InfoSchema 接成
// 能跑的 Rust；从零搭这条链路是本任务 writes 清单之外的生产能力缺口，不在这里补。
// Go 用例真正断言的是 `core.FlattenPhysicalPlan` 这一个函数的行为：给定一棵物理计划树，
// 输出按深度优先展开的 `FlatOperator` 列表，且 Build/Probe（HashJoin）、Seed/Recursive
// Part（CTE）两类特殊子节点必须打上正确的 Label，每个节点的 Level/IsLastChild/ChildrenIdx
// 必须反映原树结构。这个函数在本仓库里是完全独立于 optimizer 的真实实现（见
// `pkg/planner/core/flat_plan.rs`），直接操作 `PlanNode`/`PlanKind`（`common_plans.rs`），
// 而不是 `base::PhysicalPlan` trait object 树。因此这里改为直接手工搭建
// `PlanNode`/`PlanKind` 物理计划树（形状对应 Go testdata 里的
// `select sum(t.a) from t join t2` 和递归 CTE 两个关键用例），喂给真实的
// `FlattenPhysicalPlan`，验证同一段生产代码的展开顺序、Label、Level、IsLastChild、
// ChildrenIdx 与 `ExplainFlatPlanInRowFormat` 的树形前缀渲染，而不是简化成空断言。

use astersql_planner_core::{
    ExplainFlatPlanInRowFormat, FlattenPhysicalPlan, OperatorLabel, PlanKind, PlanNode, StoreType,
};

/// 构造叶子 TableScan 节点，便于拼装 TableReader / Join / CTE 子树。
fn leaf(id: i32, table: &str) -> PlanNode {
    PlanNode::New(
        id,
        PlanKind::TableScan {
            table: table.to_owned(),
        },
        Vec::new(),
    )
}

// test_flatten_single_table_scan_plan 对应 Go testdata 里的 "select * from t"：单链
// TableReader -> TableScan，验证最简单的形状下 flatten 出的顺序、Level、Label 全部正确。
/// 回归：单表 TableReader→TableScan 扁平化后的 Level / Label / ChildrenIdx。
#[test]
fn test_flatten_single_table_scan_plan() {
    let root = PlanNode::New(1, PlanKind::TableReader, vec![leaf(2, "t")]);

    let flat = FlattenPhysicalPlan(Some(&root), false).expect("non-empty plan must flatten");
    assert_eq!(flat.Main.len(), 2);

    assert_eq!(flat.Main[0].Origin.id, 1);
    assert!(flat.Main[0].IsRoot);
    assert_eq!(flat.Main[0].Level, 0);
    assert_eq!(flat.Main[0].Label, OperatorLabel::Empty);
    assert_eq!(flat.Main[0].ChildrenIdx, vec![1]);
    assert!(flat.Main[0].IsLastChild);

    assert_eq!(flat.Main[1].Origin.id, 2);
    assert!(!flat.Main[1].IsRoot);
    assert_eq!(flat.Main[1].Level, 1);
    assert!(flat.Main[1].IsLastChild);
    assert!(flat.Main[1].ChildrenIdx.is_empty());
}

// test_flatten_hash_join_labels_build_and_probe_sides 对应 Go testdata 里的
// "select sum(t.a) from t join t2"：HashJoin 的两个子节点必须按 `inner_child`
// 分别打上 BuildSide/ProbeSide 标签，且 build_side_first=true 时驱动侧要排到前面、
// `NeedReverseDriverSide` 要为 true。
/// 回归：HashJoin 的 Build/Probe Label，以及 build_side_first 对驱动侧顺序的影响。
#[test]
fn test_flatten_hash_join_labels_build_and_probe_sides() {
    let probe = PlanNode::New(2, PlanKind::TableReader, vec![leaf(3, "t")]);
    let build = PlanNode::New(4, PlanKind::TableReader, vec![leaf(5, "t2")]);
    let join = PlanNode::New(
        1,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![("t.a".to_owned(), "t2.a".to_owned())],
        },
        vec![probe, build],
    );

    // build_side_first = false：保持子节点原始顺序（Probe 在前，Build 在后）。
    let flat = FlattenPhysicalPlan(Some(&join), false).expect("join plan must flatten");
    assert_eq!(flat.Main[0].ChildrenIdx.len(), 2);
    let probe_idx = flat.Main[0].ChildrenIdx[0];
    let build_idx = flat.Main[0].ChildrenIdx[1];
    assert_eq!(flat.Main[probe_idx].Label, OperatorLabel::ProbeSide);
    assert_eq!(flat.Main[build_idx].Label, OperatorLabel::BuildSide);
    assert_eq!(flat.Main[probe_idx].Origin.id, 2);
    assert_eq!(flat.Main[build_idx].Origin.id, 4);
    // Go `flattenRecursively` marks this shape for display-time reversal when
    // callers request the original Probe/Build traversal order.
    assert!(flat.Main[0].NeedReverseDriverSide);

    // build_side_first = true：驱动侧（Build）必须排到子节点顺序的最前面，
    // `NeedReverseDriverSide` 此时不应设置：Go 已经在 flatten 阶段交换了
    // 两个子节点，后续展示无需再次反转。
    let flat = FlattenPhysicalPlan(Some(&join), true).expect("join plan must flatten");
    assert!(!flat.Main[0].NeedReverseDriverSide);
    let first_child_idx = flat.Main[0].ChildrenIdx[0];
    let second_child_idx = flat.Main[0].ChildrenIdx[1];
    assert_eq!(flat.Main[first_child_idx].Label, OperatorLabel::BuildSide);
    assert_eq!(flat.Main[first_child_idx].Origin.id, 4);
    assert_eq!(flat.Main[second_child_idx].Label, OperatorLabel::ProbeSide);
    assert_eq!(flat.Main[second_child_idx].Origin.id, 2);
}

// test_flatten_cte_labels_seed_and_recursive_parts 对应 Go testdata 里的递归 CTE 用例
// "WITH RECURSIVE cte (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM cte WHERE n < 5) ..."：
// CTE 节点的两个子节点必须分别打上 SeedPart/RecursivePart 标签。
/// 回归：递归 CTE（公用表表达式）的 SeedPart / RecursivePart 标签。
#[test]
fn test_flatten_cte_labels_seed_and_recursive_parts() {
    let seed = PlanNode::New(2, PlanKind::Dual, Vec::new());
    let recursive = PlanNode::New(
        3,
        PlanKind::Selection {
            conditions: vec!["n < 5".to_owned()],
        },
        Vec::new(),
    );
    let cte = PlanNode::New(1, PlanKind::CTE { storage_id: 1 }, vec![seed, recursive]);

    let flat = FlattenPhysicalPlan(Some(&cte), false).expect("CTE plan must flatten");
    assert_eq!(flat.Main.len(), 3);
    let seed_idx = flat.Main[0].ChildrenIdx[0];
    let recursive_idx = flat.Main[0].ChildrenIdx[1];
    assert_eq!(flat.Main[seed_idx].Label, OperatorLabel::SeedPart);
    assert_eq!(flat.Main[recursive_idx].Label, OperatorLabel::RecursivePart);
    assert!(flat.Main[recursive_idx].IsLastChild);
    assert!(!flat.Main[seed_idx].IsLastChild);
}

// test_flatten_join_of_ctes_tracks_level_and_children_idx 对应 Go testdata 里
// "with cte1 as (...), cte2 as (...) select * from cte1 join cte2 on cte1.a = cte2.a"：
// 两个 CTE 分别作为一个 HashJoin 的两个子树，验证多层嵌套下 Level/ChildrenIdx/IsLastChild
// 仍然正确地按深度优先顺序展开（cte1 子树先展开完，再展开 cte2 子树，最后是 join 根）。
/// 回归：两个 CTE 子树经 HashJoin 嵌套后的深度优先 Level / ChildrenIdx。
#[test]
fn test_flatten_join_of_ctes_tracks_level_and_children_idx() {
    let cte1 = PlanNode::New(
        2,
        PlanKind::CTE { storage_id: 1 },
        vec![PlanNode::New(3, PlanKind::TableReader, vec![leaf(4, "t")])],
    );
    let cte2 = PlanNode::New(
        5,
        PlanKind::CTE { storage_id: 2 },
        vec![PlanNode::New(6, PlanKind::TableReader, vec![leaf(7, "t2")])],
    );
    let join = PlanNode::New(
        1,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![("cte1.a".to_owned(), "cte2.a".to_owned())],
        },
        vec![cte1, cte2],
    );

    let flat = FlattenPhysicalPlan(Some(&join), false).expect("join-of-CTEs plan must flatten");
    // 深度优先展开顺序：join(0) -> cte1(1) -> reader(2) -> scan(3) -> cte2(4) -> reader(5) -> scan(6)。
    let ids: Vec<i32> = flat.Main.iter().map(|op| op.Origin.id).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7]);

    let levels: Vec<usize> = flat.Main.iter().map(|op| op.Level).collect();
    assert_eq!(levels, vec![0, 1, 2, 3, 1, 2, 3]);

    // 只有每条链最深的叶子和 join 根的最后一个子节点（cte2 一侧）在各自层级里是 last child。
    let is_last: Vec<bool> = flat.Main.iter().map(|op| op.IsLastChild).collect();
    assert_eq!(is_last, vec![true, false, true, true, true, true, true]);

    assert_eq!(flat.Main[0].ChildrenIdx, vec![1, 4]);
    assert_eq!(flat.Main[1].ChildrenIdx, vec![2]);
    assert_eq!(flat.Main[4].ChildrenIdx, vec![5]);
}

// test_explain_flat_plan_in_row_format_renders_tree_prefix_and_label 对应 Go
// `FlatPhysicalOperatorForTest` 里 `TextTreeIndent` 字段真正的生产来源：
// `ExplainFlatPlanInRowFormat` 按 Level/IsLastChild 渲染树形连接符（"├─"/"└─"），并把
// HashJoin 的 Build/Probe 标签追加在 ExplainID 后面，这里直接验证渲染出的第一列文本。
/// 回归：Explain 行格式中的树形前缀与 Build/Probe 标签文本。
#[test]
fn test_explain_flat_plan_in_row_format_renders_tree_prefix_and_label() {
    let probe = PlanNode::New(2, PlanKind::TableReader, vec![leaf(3, "t")]);
    let build = PlanNode::New(4, PlanKind::TableReader, vec![leaf(5, "t2")]);
    let join = PlanNode::New(
        1,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![("t.a".to_owned(), "t2.a".to_owned())],
        },
        vec![probe, build],
    );

    let flat = FlattenPhysicalPlan(Some(&join), false).expect("join plan must flatten");
    let rows = ExplainFlatPlanInRowFormat(&flat, "row", false);
    assert_eq!(rows.len(), flat.Main.len());

    assert_eq!(rows[0][0], "HashJoin_1");
    assert_eq!(rows[1][0], "├─TableReader_2(Probe)");
    assert_eq!(rows[2][0], "  └─TableScan_3");
    assert_eq!(rows[3][0], "└─TableReader_4(Build)");
    assert_eq!(rows[4][0], "  └─TableScan_5");

    assert_eq!(flat.Main[0].StoreType, StoreType::Root);
}

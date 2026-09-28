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

// 执行计划编码、归一化与 Shuffle 相关单元测试。
//
// 覆盖树形/扁平计划编码一致性、归一化 digest（忽略 id/行数/算子细节）、
// 写计划取 Select 子树、细粒度 Shuffle 包装 Window，以及 probe 计数与内存占用。

use crate::{
    AsSctx, EncodeFlatPlan, EncodePlan, FlattenPhysicalPlan, JoinType, NormalizeFlatPlan,
    NormalizePlan, PlanKind, PlanNode, PlanTask, ShuffleConfig, getActualProbeCntFromProbeParents,
    getEstimatedProbeCntFromProbeParents, getSelectPlan, optimizeByShuffle,
};

/// 构造简单 TableScan 节点，默认估计行数为 10。
fn scan(id: i32, table: &str) -> PlanNode {
    let mut plan = PlanNode::New(
        id,
        PlanKind::TableScan {
            table: table.to_owned(),
        },
        Vec::new(),
    );
    plan.estimated_rows = 10.0;
    plan.operator_info = format!("table:{table}");
    plan
}

/// 树形 EncodePlan 与扁平 EncodeFlatPlan 应对同一 HashJoin 产出相同字符串。
#[test]
fn test_encode_plan_and_flat_plan_are_identical() {
    let plan = PlanNode::New(
        1,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![("t1.a".to_owned(), "t2.a".to_owned())],
        },
        vec![scan(2, "t1"), scan(3, "t2")],
    );
    let flat = FlattenPhysicalPlan(Some(&plan), false).expect("physical plan must flatten");
    let encoded_tree = EncodePlan(Some(&plan));
    let encoded_flat = EncodeFlatPlan(&flat);
    assert_eq!(encoded_tree, encoded_flat);
    assert!(encoded_tree.contains("HashJoin"));
    assert!(encoded_tree.contains("Build"));
    assert!(encoded_tree.contains("Probe"));
}

/// 归一化 digest 应忽略节点 id、估计行数与 operator_info，仅反映计划形状。
#[test]
fn test_normalized_digest_ignores_ids_rows_and_operator_details() {
    let mut first = scan(1, "t");
    first.estimated_rows = 10.0;
    first.operator_info = "range:[1,1]".to_owned();
    let mut second = scan(999, "t");
    second.estimated_rows = 20_000.0;
    second.operator_info = "range:[2,2]".to_owned();

    let (first_normalized, first_digest) = NormalizePlan(Some(&first));
    let (second_normalized, second_digest) = NormalizePlan(Some(&second));
    assert_eq!(first_normalized, second_normalized);
    assert_eq!(first_digest, second_digest);

    // 不同算子种类应得到不同 digest。
    let different = PlanNode::New(1, PlanKind::IndexReader, vec![scan(2, "t")]);
    let (_, different_digest) = NormalizePlan(Some(&different));
    assert_ne!(first_digest, different_digest);

    let flat = FlattenPhysicalPlan(Some(&first), false).unwrap();
    let (flat_normalized, flat_digest) = NormalizeFlatPlan(&flat);
    assert_eq!(first_normalized, flat_normalized);
    assert_eq!(first_digest, flat_digest);
}

/// 空输入编码为空串；Insert 取第一个 Select 子计划；无子节点则返回 None。
#[test]
fn test_write_plan_select_child_and_empty_inputs() {
    assert_eq!(EncodePlan(None), "");
    assert!(getSelectPlan(&PlanNode::New(1, PlanKind::Dual, vec![])).is_some());

    let insert = PlanNode::New(1, PlanKind::Insert, vec![scan(2, "source")]);
    assert_eq!(
        getSelectPlan(&insert).unwrap().kind,
        PlanKind::TableScan {
            table: "source".to_owned()
        }
    );
    let empty_insert = PlanNode::New(1, PlanKind::Insert, Vec::new());
    assert!(getSelectPlan(&empty_insert).is_none());
}

/// 细粒度 Shuffle：Window 子树应被 ShuffleReceiver/Shuffle 包装，并保留 stream_count。
#[test]
fn test_fine_grained_shuffle_wraps_window_child_and_preserves_stream_count() {
    let sorted_scan = PlanNode::New(20, PlanKind::Sort, vec![scan(2, "t")]);
    let window = PlanNode::New(
        1,
        PlanKind::Window {
            functions: vec!["partition-by:c".to_owned()],
        },
        vec![sorted_scan],
    );
    let task = optimizeByShuffle(
        PlanTask::New(window),
        &ShuffleConfig {
            enabled: true,
            stream_count: 8,
        },
    );
    let window = task.plan.unwrap();
    assert!(matches!(window.kind, PlanKind::Window { .. }));
    assert!(matches!(
        window.children[0].kind,
        PlanKind::ShuffleReceiver { .. }
    ));
    assert!(window.children[0].operator_info.is_empty());
    assert!(matches!(
        window.children[0].children[0].kind,
        PlanKind::Shuffle { .. }
    ));

    // Go only enables fine-grained shuffle when the physical child is Sort.
    let unsorted = PlanNode::New(
        4,
        PlanKind::Window {
            functions: vec!["partition-by:c".to_owned()],
        },
        vec![scan(5, "t")],
    );
    let unchanged = optimizeByShuffle(
        PlanTask::New(unsorted.clone()),
        &ShuffleConfig {
            enabled: true,
            stream_count: 8,
        },
    );
    assert_eq!(unchanged.plan, Some(unsorted));

    // 关闭 Shuffle 时普通扫描计划应保持不变。
    let unchanged = optimizeByShuffle(
        PlanTask::New(scan(3, "t")),
        &ShuffleConfig {
            enabled: false,
            stream_count: 8,
        },
    );
    assert!(matches!(
        unchanged.plan.unwrap().kind,
        PlanKind::TableScan { .. }
    ));

    let empty = PlanTask {
        plan: None,
        invalid: false,
    };
    assert_eq!(
        optimizeByShuffle(empty.clone(), &ShuffleConfig::default()),
        empty
    );

    let low_ndv_sort = PlanNode::New(21, PlanKind::Sort, vec![scan(22, "one-row")]);
    let mut low_ndv_window = PlanNode::New(
        23,
        PlanKind::Window { functions: vec![] },
        vec![low_ndv_sort],
    );
    low_ndv_window.children[0].children[0].estimated_rows = 1.0;
    let low_ndv = optimizeByShuffle(
        PlanTask::New(low_ndv_window.clone()),
        &ShuffleConfig {
            enabled: true,
            stream_count: 8,
        },
    );
    assert_eq!(low_ndv.plan, Some(low_ndv_window));

    let stream_agg = PlanNode::New(
        24,
        PlanKind::StreamAgg,
        vec![PlanNode::New(25, PlanKind::Sort, vec![scan(26, "agg")])],
    );
    let stream_agg = optimizeByShuffle(
        PlanTask::New(stream_agg),
        &ShuffleConfig {
            enabled: true,
            stream_count: 3,
        },
    )
    .plan
    .unwrap();
    assert!(matches!(
        stream_agg.children[0].kind,
        PlanKind::ShuffleReceiver { .. }
    ));

    let left = PlanNode::New(30, PlanKind::Sort, vec![scan(31, "left")]);
    let right = PlanNode::New(32, PlanKind::Sort, vec![scan(33, "right")]);
    let merge = PlanNode::New(
        34,
        PlanKind::MergeJoin {
            join_type: JoinType::InnerJoin,
            keys: vec![("left.a".to_owned(), "right.a".to_owned())],
        },
        vec![left, right],
    );
    let merge = optimizeByShuffle(
        PlanTask::New(merge),
        &ShuffleConfig {
            enabled: true,
            stream_count: 4,
        },
    )
    .plan
    .unwrap();
    assert_eq!(merge.children.len(), 2);
    assert!(
        merge
            .children
            .iter()
            .all(|child| matches!(child.kind, PlanKind::ShuffleReceiver { .. }))
    );
}

/// Probe 父节点估计/实际行数乘积聚合；UnionAll 内存占用大于单节点 sizeof。
#[test]
fn test_probe_counts_and_memory_usage_follow_plan_tree() {
    let mut first = scan(1, "a");
    first.probe_count = 2.5;
    first.actual_rows = Some(3);
    let mut second = scan(2, "b");
    second.probe_count = 4.0;
    second.actual_rows = Some(5);
    let mut outer = first.clone();
    outer.estimated_rows = 7.0;
    outer.actual_rows = Some(11);
    let mut index_join = PlanNode::New(
        10,
        PlanKind::IndexJoin { keys: vec![] },
        vec![outer, second.clone()],
    );
    index_join.build_side = Some(1);
    assert_eq!(
        getEstimatedProbeCntFromProbeParents(&[index_join.clone()]),
        7.0
    );
    assert_eq!(getActualProbeCntFromProbeParents(&[index_join]), 11);

    // Go ignores non-Apply/non-index-join parents entirely.
    assert_eq!(
        getEstimatedProbeCntFromProbeParents(&[first.clone(), second.clone()]),
        1.0
    );
    assert_eq!(
        getActualProbeCntFromProbeParents(&[first.clone(), second.clone()]),
        1
    );

    let parent = PlanNode::New(
        3,
        PlanKind::UnionAll { partition: false },
        vec![first, second],
    );
    assert!(parent.MemoryUsage() > std::mem::size_of::<PlanNode>() as i64);
    assert_eq!(parent.clone(), parent);
}

#[test]
fn test_as_sctx_preserves_go_error_contract() {
    assert_eq!(
        AsSctx(None).unwrap_err(),
        "the current PlanContext cannot be converted to sessionctx.Context"
    );
}

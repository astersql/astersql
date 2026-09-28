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

// 优化器相关辅助逻辑的单元测试。
//
// 覆盖 MPP（大规模并行处理）十进制/整型键公共类型协商、细粒度 shuffle
// 在 Window 上的附着、TiFlash HashJoin V2 可用性矩阵，以及逻辑规则标志位
// 与规则列表的对齐性。

use crate::optimizer::{DoOptimize as DoOptimizeCompact, OptimizeOptions};
use crate::optimizer_runtime::{LOGICAL_RULE_FLAGS, LOGICAL_RULES};
use crate::task::{
    FieldType, PlanKind, PlanNode, Task, TypeCode, attach2TaskForMPP4PhysicalWindow,
    convertPartitionKeysIfNeed4PhysicalHashJoin, negotiateCommonType,
};
use crate::{PlanKind as CompactPlanKind, PlanNode as CompactPlanNode, PlannerContext};
use base_dependency::JoinType;
use physicalop_dependency::{CanUseHashJoinV2, IsGAForHashJoinV2};
use std::collections::HashSet;

/// 构造 Decimal FieldType 测试夹具。
fn decimal(flen: i32, decimal: i32) -> FieldType {
    FieldType {
        code: TypeCode::Decimal,
        flen,
        decimal,
        unsigned: false,
    }
}

/// 断言 negotiateCommonType 对 Decimal 的公共类型与双侧转换标志。
fn assert_decimal_common(
    left: FieldType,
    right: FieldType,
    expected: FieldType,
    conversions: (bool, bool),
) {
    let (common, left_convert, right_convert) = negotiateCommonType(&left, &right);
    assert_eq!(common, expected);
    assert_eq!((left_convert, right_convert), conversions);
}

#[test]
/// 对齐 Go 矩阵：MPP Decimal 键公共类型协商用例。
fn test_mpp_decimal_convert_matches_go_matrix() {
    let cases = [
        (5, 9, 5, 8, false, false, 5, 9),
        (5, 8, 5, 9, false, false, 5, 9),
        (0, 8, 0, 11, true, false, 0, 11),
        (0, 16, 0, 11, false, false, 0, 16),
        (5, 9, 4, 9, true, true, 5, 10),
        (5, 8, 4, 9, true, true, 5, 10),
        (5, 9, 4, 8, false, true, 5, 9),
        (10, 16, 0, 11, true, true, 10, 21),
        (5, 19, 0, 20, false, true, 5, 25),
        (20, 20, 0, 60, true, true, 20, 65),
        (20, 40, 0, 60, false, true, 20, 65),
        (0, 40, 0, 60, false, false, 0, 60),
    ];
    for (ld, lf, rd, rf, lc, rc, cd, cf) in cases {
        assert_decimal_common(decimal(lf, ld), decimal(rf, rd), decimal(cf, cd), (lc, rc));
    }
}

/// 构造整型 FieldType 测试夹具。
fn integer(code: TypeCode, flen: i32, unsigned: bool) -> FieldType {
    FieldType {
        code,
        flen,
        decimal: 0,
        unsigned,
    }
}

#[test]
/// 校验 Join 键类型提升，以及类型转换后细粒度 shuffle 被失效。
fn test_mpp_join_key_type_convert_and_shuffle_invalidation() {
    let tiny = integer(TypeCode::Int, 4, false);
    let unsigned_tiny = integer(TypeCode::UInt, 3, true);
    let bigint = integer(TypeCode::Int, 20, false);
    let unsigned_bigint = integer(TypeCode::UInt, 20, true);

    assert_eq!(
        negotiateCommonType(&tiny, &tiny),
        (tiny.clone(), false, false)
    );
    let (common, left, right) = negotiateCommonType(&tiny, &unsigned_tiny);
    assert_eq!(common, bigint);
    assert!((left && right));

    let (common, left, right) = negotiateCommonType(&unsigned_bigint, &tiny);
    assert_eq!(common, decimal(20, 0));
    assert!((left && right));

    let mut left_task = Task::Mpp {
        plan: Some(PlanNode {
            flags: crate::task::PlanFlags {
                fine_grained_shuffle: true,
                ..Default::default()
            },
            ..PlanNode::new(PlanKind::TableScan)
        }),
        partition_keys: vec![tiny],
        warnings: Default::default(),
    };
    let mut right_task = Task::Mpp {
        plan: Some(PlanNode {
            flags: crate::task::PlanFlags {
                fine_grained_shuffle: true,
                ..Default::default()
            },
            ..PlanNode::new(PlanKind::TableScan)
        }),
        partition_keys: vec![unsigned_bigint],
        warnings: Default::default(),
    };
    convertPartitionKeysIfNeed4PhysicalHashJoin(&mut left_task, &mut right_task);
    for task in [&left_task, &right_task] {
        assert_eq!(task.plan().unwrap().flags.fine_grained_shuffle, false);
    }
}

#[test]
/// Window 附着到 MPP ExchangeReceiver 时标记细粒度 shuffle；Root 任务则无效。
fn test_handle_fine_grained_shuffle_marks_mpp_window_only() {
    let window = PlanNode::new(PlanKind::Window);
    let task = Task::mpp(PlanNode::new(PlanKind::ExchangeReceiver));
    let attached = attach2TaskForMPP4PhysicalWindow(window, task);
    let plan = attached.plan().expect("MPP window must produce a plan");
    assert_eq!(plan.kind, PlanKind::Window);
    assert!(plan.flags.fine_grained_shuffle);
    assert_eq!(plan.children[0].kind, PlanKind::ExchangeReceiver);

    let invalid = attach2TaskForMPP4PhysicalWindow(
        PlanNode::new(PlanKind::Window),
        Task::root(PlanNode::new(PlanKind::TableReader)),
    );
    assert!(invalid.is_invalid());
}

#[test]
/// TiFlash HashJoin V2 可用性与 GA 判定矩阵。
fn test_can_tiflash_use_hash_join_v2_matrix() {
    let keys = vec![expression_dependency::Column::default()];
    assert!(CanUseHashJoinV2(JoinType::InnerJoin, &keys, &[], &[]));
    assert!(IsGAForHashJoinV2(JoinType::SemiJoin, &keys, &[], &[]));
    assert!(!CanUseHashJoinV2(JoinType::InnerJoin, &[], &[], &[]));
    assert!(!CanUseHashJoinV2(JoinType::InnerJoin, &keys, &[true], &[]));
    assert!(!CanUseHashJoinV2(JoinType::InnerJoin, &keys, &[], &keys));
}

#[test]
/// 逻辑规则列表与标志位一一对应，且每位为互不相交的二次幂。
fn test_opt_rule_list_flag_alignment() {
    assert_eq!(LOGICAL_RULES.len(), LOGICAL_RULE_FLAGS.len());
    let mut seen = HashSet::new();
    for &flag in LOGICAL_RULE_FLAGS {
        assert_ne!(flag, 0);
        assert_eq!(flag & (flag - 1), 0);
        assert!(seen.insert(flag), "duplicate logical rule flag {flag}");
    }
    assert_eq!(
        seen.len(),
        (rule_dependency::FLAG_FULLTEXT_INDEX_RESOLVE_REJECT.ilog2() + 1) as usize
    );
}

#[test]
/// Go only descends through an Apply's outer child when enabling parallel Apply.
fn parallel_apply_does_not_enable_nested_inner_apply() {
    let leaf = || CompactPlanNode::New(1, CompactPlanKind::TableScan { table: "t".into() }, vec![]);
    let inner_apply = CompactPlanNode::New(2, CompactPlanKind::Apply, vec![leaf(), leaf()]);
    let root = CompactPlanNode::New(3, CompactPlanKind::Apply, vec![leaf(), inner_apply]);
    let options = OptimizeOptions {
        enable_parallel_apply: true,
        ..Default::default()
    };

    let (plan, _, _) = DoOptimizeCompact(&PlannerContext::default(), 0, root, &options).unwrap();

    assert!(plan.operator_info.contains("parallel"));
    assert!(!plan.children[1].operator_info.contains("parallel"));
}

#[test]
/// Probe-parent counts are propagated only into an Apply/IndexJoin inner child.
fn probe_parent_counts_follow_index_join_inner_side() {
    let mut join = CompactPlanNode::New(
        3,
        CompactPlanKind::IndexJoin { keys: vec![] },
        vec![
            CompactPlanNode::New(
                1,
                CompactPlanKind::TableScan {
                    table: "outer".into(),
                },
                vec![],
            ),
            CompactPlanNode::New(
                2,
                CompactPlanKind::TableScan {
                    table: "inner".into(),
                },
                vec![],
            ),
        ],
    );
    join.estimated_rows = 7.0;

    let (plan, _, _) = DoOptimizeCompact(
        &PlannerContext::default(),
        0,
        join,
        &OptimizeOptions::default(),
    )
    .unwrap();

    assert_eq!(plan.children[0].probe_count, 1.0);
    assert_eq!(plan.children[1].probe_count, 7.0);
}

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

// 物理属性与相关工具的单元测试。
//
// 覆盖：基于 FD 等价类的 Exchange 强制、分区类型规则、排序前缀/部分有序、
// HashCode 缓存与区分字段、统计 Scale/Limit、以及 TaskType 字符串。

use std::collections::HashMap;

use crate::*;

/// 构造带 UniqueID 的测试列。
fn column(unique_id: i64) -> expression::Column {
    expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        unique_id,
        unique_id,
        0,
    )
}

/// 构造 CollateID=0 的 MPP 分区列。
fn partition_column(unique_id: i64) -> MPPPartitionColumn {
    MPPPartitionColumn {
        Col: column(unique_id),
        CollateID: 0,
    }
}

/// 由整数切片构造 FastIntSet。
fn set(values: &[i32]) -> intset::FastIntSet {
    intset::NewFastIntSet(values.to_vec())
}

/// 构造接近 TPC-H Q3 的函数依赖图，用于等价类 Exchange 用例。
fn build_tpch_q3_fd() -> funcdep::FDSet {
    let mut dependencies = funcdep::FDSet::default();
    dependencies.AddEquivalence(set(&[1, 10]), set(&[1, 10]));
    dependencies.AddStrictFunctionalDependency(set(&[1]), set(&[2, 3, 4, 5, 6, 8]));
    dependencies.AddStrictFunctionalDependency(set(&[]), set(&[7]));
    dependencies.AddStrictFunctionalDependency(set(&[9]), set(&[10, 11, 12, 13, 14, 15, 16, 17]));
    dependencies.AddStrictFunctionalDependency(
        set(&[10, 21]),
        set(&[
            19, 20, 22, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33,
        ]),
    );
    dependencies.AddEquivalenceUnion(set(&[9, 18]));
    dependencies.AddEquivalenceUnion(set(&[1, 10]));
    dependencies
}

/// 简单等价：2↔4、3↔4。
fn build_fd() -> funcdep::FDSet {
    let mut dependencies = funcdep::FDSet::default();
    dependencies.AddEquivalence(set(&[2]), set(&[4]));
    dependencies.AddEquivalence(set(&[3]), set(&[4]));
    dependencies
}

/// 简单等价：2↔4、2↔5。
fn build_fd2() -> funcdep::FDSet {
    let mut dependencies = funcdep::FDSet::default();
    dependencies.AddEquivalence(set(&[2]), set(&[4]));
    dependencies.AddEquivalence(set(&[2]), set(&[5]));
    dependencies
}

/// 与 Go 对齐：基于等价闭包判断是否仍需强制 Exchange。
#[test]
fn need_enforce_exchanger_with_hash_by_equivalence_matches_go_cases() {
    let cases = vec![
        (
            build_tpch_q3_fd(),
            vec![
                partition_column(18),
                partition_column(13),
                partition_column(16),
            ],
            vec![partition_column(9)],
            false,
        ),
        (
            build_tpch_q3_fd(),
            vec![
                partition_column(18),
                partition_column(13),
                partition_column(16),
            ],
            vec![partition_column(9), partition_column(13)],
            false,
        ),
        (
            build_tpch_q3_fd(),
            vec![
                partition_column(18),
                partition_column(13),
                partition_column(16),
            ],
            vec![partition_column(9), partition_column(17)],
            true,
        ),
        (
            build_tpch_q3_fd(),
            vec![
                partition_column(18),
                partition_column(13),
                partition_column(16),
            ],
            vec![partition_column(1), partition_column(17)],
            true,
        ),
        (
            build_fd(),
            vec![
                partition_column(1),
                partition_column(2),
                partition_column(3),
            ],
            vec![
                partition_column(1),
                partition_column(2),
                partition_column(4),
                partition_column(5),
            ],
            true,
        ),
        (
            build_fd2(),
            vec![partition_column(1), partition_column(2)],
            vec![
                partition_column(1),
                partition_column(2),
                partition_column(5),
            ],
            false,
        ),
    ];

    for (dependencies, required, supplied, expected) in cases {
        let property = PhysicalProperty {
            MPPPartitionCols: required,
            ..PhysicalProperty::default()
        };
        assert_eq!(
            property.NeedMPPExchangeByEquivalence(&supplied, &dependencies),
            expected
        );
    }
}

/// Any/Broadcast/Single/Hash 分区规则下 NeedEnforceExchanger 行为。
#[test]
fn exchanger_rules_cover_any_broadcast_single_and_hash() {
    let mut property = PhysicalProperty::default();
    assert!(!NeedEnforceExchanger(HashType, &[], &property, None));

    property.MPPPartitionTp = BroadcastType;
    assert!(NeedEnforceExchanger(BroadcastType, &[], &property, None));

    property.MPPPartitionTp = SinglePartitionType;
    assert!(!NeedEnforceExchanger(
        SinglePartitionType,
        &[],
        &property,
        None
    ));
    assert!(NeedEnforceExchanger(HashType, &[], &property, None));

    property.MPPPartitionTp = HashType;
    property.MPPPartitionCols = vec![partition_column(1), partition_column(2)];
    assert!(!NeedEnforceExchanger(
        HashType,
        &[partition_column(1), partition_column(2)],
        &property,
        None
    ));
    // 列顺序不同视为不匹配。
    assert!(NeedEnforceExchanger(
        HashType,
        &[partition_column(2), partition_column(1)],
        &property,
        None
    ));
}

/// 前缀匹配、部分有序与 KeepOrder 辅助语义。
#[test]
fn order_helpers_preserve_prefix_partial_and_advisory_semantics() {
    let ascending = SortItem {
        Col: column(1),
        Desc: false,
    };
    let descending = SortItem {
        Col: column(2),
        Desc: true,
    };
    let prefix = PhysicalProperty {
        SortItems: vec![ascending.clone()],
        ..PhysicalProperty::default()
    };
    let full = PhysicalProperty {
        SortItems: vec![ascending.clone(), descending.clone()],
        ..PhysicalProperty::default()
    };
    assert!(prefix.IsPrefix(&full));
    assert_eq!(full.AllSameOrder(), (false, false));
    assert!(full.NeedKeepOrder());

    let partial = PhysicalProperty {
        PartialOrderInfo: Some(PartialOrderInfo {
            SortItems: vec![descending.clone()],
        }),
        ..PhysicalProperty::default()
    };
    assert!(partial.NeedKeepOrder());
    assert!(partial.GetSortDescForKeepOrder());
    assert_eq!(partial.GetSortItemsForKeepOrder().len(), 1);
}

/// HashCode 缓存稳定，且 CanAddEnforcer / AdvisorySortItems 会改变指纹。
#[test]
fn hash_code_covers_all_go_discriminating_fields_and_is_cached() {
    let base = PhysicalProperty {
        SortItems: vec![SortItem {
            Col: column(1),
            Desc: false,
        }],
        TaskTp: RootTaskType,
        ExpectedCnt: 10.0,
        ..PhysicalProperty::default()
    };
    let first = base.HashCode();
    assert_eq!(first, base.HashCode());

    let enforced = PhysicalProperty {
        CanAddEnforcer: true,
        ..base.CloneEssentialFields()
    };
    assert_ne!(first, enforced.HashCode());

    let advisory = PhysicalProperty {
        AdvisorySortItems: vec![SortItem {
            Col: column(2),
            Desc: true,
        }],
        ..base.CloneEssentialFields()
    };
    assert_ne!(first, advisory.HashCode());
}

/// 测试用线性 NDV 缩放：new = original * selected / original_rows。
fn scale_ndv(
    _variables: &variable::SessionVars,
    original_ndv: f64,
    original_rows: f64,
    selected_rows: f64,
) -> f64 {
    original_ndv * selected_rows / original_rows
}

/// Scale、GetGroupNDV4Cols、DeriveLimitStats 与 Go 行为对齐。
#[test]
fn statistics_scaling_group_lookup_and_limit_match_go() {
    SetScaleNDVFunc(Some(scale_ndv));
    let variables = variable::SessionVars::new();
    let stats = StatsInfo {
        RowCount: 100.0,
        ColNDVs: HashMap::from([(1, 80.0), (2, 20.0)]),
        StatsVersion: 2,
        GroupNDVs: vec![GroupNDV {
            Cols: vec![1, 2],
            NDV: 90.0,
        }],
        ..StatsInfo::default()
    };

    let scaled = stats.Scale(&variables, 0.5);
    assert_eq!(scaled.RowCount, 50.0);
    assert_eq!(scaled.ColNDVs[&1], 40.0);
    assert_eq!(scaled.GroupNDVs[0].NDV, 45.0);
    assert_eq!(scaled.StatsVersion, 2);
    assert_eq!(
        stats.GetGroupNDV4Cols(&[column(2), column(1)]).unwrap().NDV,
        90.0
    );

    let limited = DeriveLimitStats(&stats, 10.0);
    assert_eq!(limited.RowCount, 10.0);
    assert_eq!(limited.ColNDVs[&1], 10.0);
    assert!(limited.GroupNDVs.is_empty());
}

/// TaskType::String 对已知与未知取值的映射。
#[test]
fn task_type_keeps_known_and_unknown_strings() {
    assert_eq!(RootTaskType.String(), "rootTask");
    assert_eq!(CopSingleReadTaskType.String(), "copSingleReadTask");
    assert_eq!(CopMultiReadTaskType.String(), "copMultiReadTask");
    assert_eq!(MppTaskType.String(), "mppTask");
    assert_eq!(TaskType(99).String(), "UnknownTaskType");
}

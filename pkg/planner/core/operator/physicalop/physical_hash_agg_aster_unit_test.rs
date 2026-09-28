// Copyright 2026 AsterSQL.

// `physical_hash_agg` 的 Aster 单元测试。
//
// 校验 `clone_agg_funcs` 会完整复制逻辑聚合描述符字段
//（Mode、DISTINCT、GroupingID、返回类型与 OrderByItems），避免物理改写污染逻辑计划。

use crate::physical_hash_agg::{clone_agg_funcs, hash_agg_cpu_cost_divisor, tiflash_pre_agg_mode};
use crate::{
    AggMppRunMode, commit_mpp_agg_candidate_column_id, mpp_agg_partition_is_satisfied,
    scalar_mpp_run_mode,
};

#[test]
fn mpp_agg_reuses_satisfied_hash_partition() {
    let mut column = expression::Column::default();
    column.UniqueID = 7;
    let partition = property::MPPPartitionColumn {
        Col: column,
        CollateID: 0,
    };

    assert!(mpp_agg_partition_is_satisfied(
        property::HashType,
        std::slice::from_ref(&partition),
        std::slice::from_ref(&partition),
    ));
    assert!(!mpp_agg_partition_is_satisfied(
        property::AnyType,
        std::slice::from_ref(&partition),
        std::slice::from_ref(&partition),
    ));

    let mut other_column = expression::Column::default();
    other_column.UniqueID = 8;
    let other_partition = property::MPPPartitionColumn {
        Col: other_column,
        CollateID: 0,
    };

    // Go treats a single partition as satisfying every grouping layout, while
    // a hash layout must be non-empty and match every required key exactly.
    assert!(mpp_agg_partition_is_satisfied(
        property::SinglePartitionType,
        &[],
        std::slice::from_ref(&partition),
    ));
    assert!(!mpp_agg_partition_is_satisfied(
        property::HashType,
        &[],
        &[],
    ));
    assert!(!mpp_agg_partition_is_satisfied(
        property::HashType,
        std::slice::from_ref(&partition),
        std::slice::from_ref(&other_partition),
    ));
    assert!(!mpp_agg_partition_is_satisfied(
        property::HashType,
        &[partition.clone(), other_partition.clone()],
        std::slice::from_ref(&partition),
    ));
    assert!(mpp_agg_partition_is_satisfied(
        property::HashType,
        &[partition.clone(), other_partition.clone()],
        &[other_partition, partition],
    ));
}

#[test]
fn mpp_agg_column_id_commits_only_winning_candidate() {
    let base = 40;
    let losing_candidate = 47;
    let winning_candidate = 44;

    assert_eq!(
        commit_mpp_agg_candidate_column_id(base, losing_candidate, false),
        base
    );
    assert_eq!(
        commit_mpp_agg_candidate_column_id(base, winning_candidate, true),
        winning_candidate
    );
}

#[test]
fn scalar_mpp_mode_matches_go_execution_location() {
    assert_eq!(scalar_mpp_run_mode(false), AggMppRunMode::MppTiDB);
    assert_eq!(scalar_mpp_run_mode(true), AggMppRunMode::MppScalar);
}

#[test]
/// 构造带齐全字段的 AggFuncDesc，克隆后逐项断言未被截断。
fn hash_agg_clones_complete_logical_descriptors() {
    // 填充 mode、分组 ID、返回类型与排序项，模拟完整逻辑描述符。
    let mut descriptor = aggregation::newAggFunc("count", Vec::new(), true).AggFuncDesc;
    descriptor.Mode = aggregation::DedupMode;
    descriptor.GroupingID = 17;
    descriptor.RetTp = Some(*expression::types::NewFieldType(
        expression::mysql::TypeLonglong,
    ));
    descriptor.OrderByItems = vec![planner_util::ByItems {
        Expr: Box::new(expression::Column::default()),
        Desc: true,
    }];

    let mut cloned = clone_agg_funcs(std::slice::from_ref(&descriptor));

    assert_eq!(cloned[0].Mode, aggregation::DedupMode);
    assert!(cloned[0].HasDistinct);
    assert_eq!(cloned[0].GroupingID, 17);
    assert_eq!(
        cloned[0]
            .RetTp
            .as_ref()
            .map(|field_type| field_type.GetType()),
        Some(expression::mysql::TypeLonglong)
    );
    assert_eq!(cloned[0].OrderByItems.len(), 1);
    assert!(cloned[0].OrderByItems[0].Desc);

    // Go's AggFuncDesc.Clone owns the nested order-by items. Mutating the
    // physical descriptor must therefore leave the logical descriptor intact.
    cloned[0].OrderByItems[0].Desc = false;
    cloned[0].GroupingID = 99;
    assert!(descriptor.OrderByItems[0].Desc);
    assert_eq!(descriptor.GroupingID, 17);
}

#[test]
fn hash_agg_cpu_divisor_matches_go_concurrency_rules() {
    assert_eq!(hash_agg_cpu_cost_divisor(true, 4, 8), (0.0, 0.0));
    assert_eq!(hash_agg_cpu_cost_divisor(false, 1, 1), (0.0, 0.0));
    assert_eq!(hash_agg_cpu_cost_divisor(false, 4, 8), (4.0, 12.0));
    assert_eq!(hash_agg_cpu_cost_divisor(false, 9, 3), (3.0, 12.0));
}

#[test]
fn tiflash_pre_aggregation_mode_matches_go_validation() {
    assert_eq!(
        tiflash_pre_agg_mode(vardef::ForcePreAggStr).unwrap(),
        tipb::TiFlashPreAggMode::ForcePreAgg
    );
    assert_eq!(
        tiflash_pre_agg_mode(vardef::AutoStr).unwrap(),
        tipb::TiFlashPreAggMode::Auto
    );
    assert_eq!(
        tiflash_pre_agg_mode(vardef::ForceStreamingStr).unwrap(),
        tipb::TiFlashPreAggMode::ForceStreaming
    );
    assert!(tiflash_pre_agg_mode("invalid").is_err());
}

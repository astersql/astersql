// Copyright 2026 AsterSQL.

use base::JoinType;
use expression::{Column, NewSchema};

use crate::physical_hash_join::{
    PhysicalHashJoin, can_tiflash_use_hash_join_v2, can_use_hash_join_v2_with_non_ga,
};

#[test]
fn repeated_hash_join_output_columns_are_deduplicated_before_resolution() {
    let source = Column::default();
    let mut output = NewSchema(vec![source.Clone(), source.Clone()]);
    let children = NewSchema(vec![source]);

    assert_eq!(
        PhysicalHashJoin::DeduplicateOutputColumns(&mut output, 2),
        1
    );
    assert_eq!(output.Columns.len(), 1);
    assert_eq!(
        PhysicalHashJoin::ResolveOutputColumns(&mut output, &children, 1),
        1
    );
    assert_eq!(output.Columns[0].Index, 0);
}

#[test]
fn non_ga_hash_join_v2_respects_feature_gate() {
    let key = expression::Column::default();
    assert!(!can_use_hash_join_v2_with_non_ga(
        JoinType::LeftOuterSemiJoin,
        std::slice::from_ref(&key),
        &[],
        &[],
        false,
    ));
    assert!(can_use_hash_join_v2_with_non_ga(
        JoinType::LeftOuterSemiJoin,
        std::slice::from_ref(&key),
        &[],
        &[],
        true,
    ));
    assert!(can_use_hash_join_v2_with_non_ga(
        JoinType::InnerJoin,
        &[key],
        &[],
        &[],
        false,
    ));
}

#[test]
fn tiflash_hash_join_v2_rejects_legacy_spill_and_unsupported_shapes() {
    assert!(can_tiflash_use_hash_join_v2(
        "optimized",
        -1,
        -1,
        0.0,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "legacy",
        -1,
        -1,
        0.0,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "optimized",
        1,
        -1,
        0.0,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "optimized",
        -1,
        1,
        0.5,
        JoinType::InnerJoin,
        true,
        false,
        false,
    ));
    assert!(!can_tiflash_use_hash_join_v2(
        "optimized",
        -1,
        -1,
        0.0,
        JoinType::LeftOuterJoin,
        true,
        false,
        false,
    ));
}

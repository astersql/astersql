// Copyright 2026 AsterSQL.

use crate::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec, get_partition_mask_offset};
use crate::joiner::{JoinType, Joiner};
use crate::row_table_builder::Value;

fn can_skip(join_type: JoinType, right_as_build_side: bool) -> bool {
    let context = HashJoinCtxV2::new(
        join_type,
        vec![0],
        vec![0],
        right_as_build_side,
        false,
        1,
        32,
        None,
    )
    .unwrap();
    let joiner = Joiner::new(join_type, true, vec![Value::Null], vec![], None, false, 32).unwrap();
    HashJoinV2Exec::new(context, joiner, vec![], vec![])
        .unwrap()
        .can_skip_probe_if_hash_table_is_empty()
}

#[test]
fn empty_hash_table_probe_skip_matches_go_build_side_matrix() {
    assert!(can_skip(JoinType::Inner, false));

    assert!(can_skip(JoinType::LeftOuter, false));
    assert!(!can_skip(JoinType::LeftOuter, true));

    assert!(!can_skip(JoinType::RightOuter, false));
    assert!(can_skip(JoinType::RightOuter, true));

    assert!(!can_skip(JoinType::Semi, false));
    assert!(can_skip(JoinType::Semi, true));

    assert!(!can_skip(JoinType::AntiSemi, false));
    assert!(!can_skip(JoinType::AntiLeftOuterSemi, true));
}

#[test]
fn single_partition_mask_offset_matches_go_trailing_zeros_contract() {
    assert_eq!(get_partition_mask_offset(1), 64);
}

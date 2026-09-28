// Copyright 2026 AsterSQL.

// HashJoin V2 可用性与跳过开关的单元测试。
//
// 覆盖等值键、连接类型与会话开关对 `CanUseHashJoinV2` /
// `IsGAForHashJoinV2` / `ShouldSkipHashJoin` 的约束。

use base::JoinType;

use crate::{CanUseHashJoinV2, IsGAForHashJoinV2, ShouldSkipHashJoin};

/// 验证 HashJoin V2 的 GA 和完整支持白名单与 Go 实现一致。
#[test]
fn hash_join_v2_requires_supported_join_and_equality_keys() {
    let keys = vec![expression::Column::default()];

    for join_type in [
        JoinType::LeftOuterJoin,
        JoinType::RightOuterJoin,
        JoinType::InnerJoin,
        JoinType::SemiJoin,
        JoinType::AntiSemiJoin,
    ] {
        assert!(IsGAForHashJoinV2(join_type, &keys, &[], &[]));
        assert!(CanUseHashJoinV2(join_type, &keys, &[], &[]));
    }

    // Go 仅在非 GA 开关允许时把这两种 outer-semi 形态纳入完整支持集。
    for join_type in [JoinType::LeftOuterSemiJoin, JoinType::AntiLeftOuterSemiJoin] {
        assert!(!IsGAForHashJoinV2(join_type, &keys, &[], &[]));
        assert!(CanUseHashJoinV2(join_type, &keys, &[], &[]));
    }

    // 无等值键、NullEQ 或 Null-Aware 键任一存在时，GA 与完整支持判定都必须拒绝。
    let na_keys = vec![expression::Column::default()];
    for (left_keys, null_eq, left_na_keys) in [
        (&[][..], &[][..], &[][..]),
        (&keys[..], &[true][..], &[][..]),
        (&keys[..], &[false, true][..], &[][..]),
        (&keys[..], &[][..], &na_keys[..]),
    ] {
        assert!(!IsGAForHashJoinV2(
            JoinType::InnerJoin,
            left_keys,
            null_eq,
            left_na_keys,
        ));
        assert!(!CanUseHashJoinV2(
            JoinType::InnerJoin,
            left_keys,
            null_eq,
            left_na_keys,
        ));
    }
}

/// 验证任一会话开关为真时都应跳过 HashJoin。
#[test]
fn skip_hash_join_honors_all_switches() {
    assert!(!ShouldSkipHashJoin(false, false));
    assert!(ShouldSkipHashJoin(true, false));
    assert!(ShouldSkipHashJoin(false, true));
    assert!(ShouldSkipHashJoin(true, true));
}

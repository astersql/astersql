// Copyright 2026 AsterSQL.

use super::index_lookup_hash_join::IndexNestedLoopHashJoin;
use super::index_lookup_join::{IndexJoinExecutorBuilder, IndexJoinLookupContent};
use super::joiner::{JoinType, Joiner};

struct EmptyBuilder;

impl IndexJoinExecutorBuilder for EmptyBuilder {
    fn build(
        &self,
        _: &[IndexJoinLookupContent],
    ) -> Result<Vec<Vec<crate::row_table_builder::Value>>, String> {
        Ok(Vec::new())
    }
}

fn executor(join_type: JoinType, keep_outer_order: bool) -> IndexNestedLoopHashJoin {
    IndexNestedLoopHashJoin::new(
        Vec::new(),
        vec![0],
        vec![0],
        Box::new(EmptyBuilder),
        Joiner::new(join_type, false, Vec::new(), Vec::new(), None, false, 8).unwrap(),
        keep_outer_order,
        1,
    )
    .unwrap()
}

#[test]
fn incremental_lookup_matches_go_join_type_and_order_gate() {
    for join_type in [
        JoinType::Inner,
        JoinType::LeftOuter,
        JoinType::RightOuter,
        JoinType::AntiSemi,
    ] {
        assert!(executor(join_type, false).support_incremental_lookup());
        assert!(!executor(join_type, true).support_incremental_lookup());
    }

    for join_type in [
        JoinType::Semi,
        JoinType::LeftOuterSemi,
        JoinType::AntiLeftOuterSemi,
    ] {
        assert!(!executor(join_type, false).support_incremental_lookup());
        assert!(!executor(join_type, true).support_incremental_lookup());
    }
}

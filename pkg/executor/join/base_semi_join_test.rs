// Copyright 2026 AsterSQL.

use crate::base_join_probe::{BaseJoinProbe, HashJoinContext};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

fn row(key: i64, payload: i64) -> Row {
    vec![Value::Int(key), Value::Int(payload)]
}

#[test]
fn matching_probe_row_does_not_mark_every_candidate_build_row_used() {
    let condition: Predicate = Arc::new(|joined| match (&joined[1], &joined[3]) {
        (Value::Int(build), Value::Int(probe)) => Ok(Some(build == probe)),
        _ => Ok(Some(false)),
    });
    let joiner = Joiner::new(
        JoinType::Semi,
        false,
        Vec::new(),
        vec![condition],
        None,
        false,
        32,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![row(1, 10), row(1, 20)],
        vec![0],
        vec![0],
        joiner,
        true,
        true,
        32,
    );
    let mut semi = BaseSemiJoin::new(BaseJoinProbe::new(context, 0), false);
    semi.base.set_chunk_for_probe(vec![row(1, 20)]).unwrap();
    semi.reset_probe_state();

    let result = semi
        .match_probe_row(0, &mut Vec::new(), crate::joiner::NaajType::Unknown)
        .unwrap();

    assert!(result.matched);
    assert_eq!(semi.base.context.build_row_used, vec![false, false]);
}

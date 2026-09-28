// Copyright 2026 AsterSQL.

use crate::base_join_probe::{HashJoinContext, KeyMode, is_key_matched, new_join_probe};
use crate::joiner::{JoinType, Joiner, Row};
use crate::row_table_builder::Value;

fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

fn inner_probe(max_chunk_size: usize) -> Box<dyn crate::base_join_probe::Probe> {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        max_chunk_size,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![row(1), row(1)],
        vec![0],
        vec![0],
        joiner,
        true,
        true,
        max_chunk_size,
    );
    new_join_probe(context, 0, JoinType::Inner, true, false).unwrap()
}

#[test]
fn set_chunk_rejects_replacing_unfinished_probe_chunk() {
    let mut probe = inner_probe(1);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows.len(), 1);
    assert!(!probe.is_current_chunk_probe_done());

    let error = probe.set_chunk_for_probe(vec![row(2)]).unwrap_err();
    assert_eq!(error, "previous chunk is not probed yet");

    assert_eq!(probe.probe().rows.len(), 1);
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
fn one_int64_key_requires_a_complete_value() {
    assert!(!is_key_matched(KeyMode::OneInt64, &[1, 2], &[1, 2]));
    assert!(!is_key_matched(KeyMode::OneInt64, &[0; 8], &[0; 7]));
    assert!(is_key_matched(KeyMode::OneInt64, &[7; 8], &[7; 8]));
}

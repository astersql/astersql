// Copyright 2026 AsterSQL.

use super::hash_join_test_util::{
    HashJoinInfo, build_hash_join_v1_exec, execute_hash_join_exec,
    execute_hash_join_exec_for_random_fail_test,
};
use super::hash_join_v1::ExecutorState;
use super::joiner::JoinType;
use super::row_table_builder::Value;

fn executor() -> super::hash_join_v1::HashJoinV1Exec {
    build_hash_join_v1_exec(&HashJoinInfo {
        join_type: JoinType::Inner,
        build_key_indices: vec![0],
        probe_key_indices: vec![0],
        build_chunks: vec![vec![vec![Value::Int(1)]]],
        probe_chunks: vec![vec![vec![Value::Int(1)]]],
        outer_is_right: false,
        build_side_is_outer: false,
        null_aware: false,
        concurrency: 1,
        max_chunk_size: 1,
        default_inner: Vec::new(),
        conditions: Vec::new(),
        children_used: None,
    })
    .unwrap()
}

#[test]
fn execution_helpers_close_executor_like_go() {
    let mut normal = executor();
    assert_eq!(execute_hash_join_exec(&mut normal).unwrap().len(), 1);
    assert_eq!(normal.state(), ExecutorState::Closed);

    let mut cancelled = executor();
    assert_eq!(
        execute_hash_join_exec_for_random_fail_test(&mut cancelled, Some(0)),
        Err("hash join cancelled".to_owned())
    );
    assert_eq!(cancelled.state(), ExecutorState::Closed);
}

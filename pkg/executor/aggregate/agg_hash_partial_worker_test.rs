// Copyright 2026 AsterSQL.

use std::sync::Arc;

use super::agg_hash_partial_worker::{HashAggPartialWorker, murmur3_sum32};
use super::agg_util::{AggKind, Aggregation, Value};

#[test]
fn partial_worker_partition_hash_matches_go_murmur3() {
    // twmb/murmur3.Sum32([]byte("hello")) in the Go implementation.
    assert_eq!(murmur3_sum32(b"hello"), 613_153_351);
}

#[test]
fn partial_worker_shuffles_group_to_the_go_worker() {
    let mut worker = HashAggPartialWorker::new(
        vec![0],
        Arc::new(vec![Aggregation::new(AggKind::Count, None)]),
        None,
    );
    worker
        .update_partial_result(&vec![vec![Value::Text("hello".into())]])
        .unwrap();

    let outputs = worker.shuffle_intermediate_data(7);

    // The encoded text key hashes to 2877188159 with twmb/murmur3.Sum32.
    assert_eq!(outputs.iter().map(|output| output.len()).sum::<usize>(), 1);
    assert_eq!(outputs[6].len(), 1);
}

// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::agg_hash_base_worker::BaseHashAggWorker;
use super::agg_util::{AggKind, Aggregation};

fn worker_with_aggregation_count(count: usize) -> BaseHashAggWorker {
    let aggregations = (0..count)
        .map(|_| Aggregation::new(AggKind::Count, None))
        .collect();
    BaseHashAggWorker::new(Arc::new(AtomicBool::new(false)), Arc::new(aggregations), 0)
}

#[test]
fn base_worker_preserves_go_chunk_size_and_partial_result_alignment() {
    let expected = [(0, 0), (1, 1), (2, 2), (3, 4), (4, 4), (5, 6)];

    for (count, aligned) in expected {
        let worker = worker_with_aggregation_count(count);
        assert_eq!(worker.max_chunk_size, 0, "aggregation count {count}");
        assert_eq!(
            worker.aligned_partial_result_len(),
            aligned,
            "aggregation count {count}"
        );
    }
}

#[test]
fn base_worker_observes_finish_signal() {
    let finish = Arc::new(AtomicBool::new(false));
    let worker = BaseHashAggWorker::new(finish.clone(), Arc::new(Vec::new()), 1);

    assert!(!worker.is_finished());
    finish.store(true, Ordering::Release);
    assert!(worker.is_finished());
}

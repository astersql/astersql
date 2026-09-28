// Copyright 2026 AsterSQL.

use super::collector::{
    Collector, LABEL_DOUBLE_WRITE, LABEL_MERGE, LABEL_SCAN, LABEL_SINGLE_WRITE,
};
use std::sync::Arc;

fn value(collector: &Collector, operation: &str, table_id: i64) -> Option<u64> {
    collector
        .collect()
        .into_iter()
        .find(|metric| metric.operation == operation && metric.table_id == table_id)
        .map(|metric| metric.value)
}

#[test]
fn write_counts_follow_go_commit_and_rollback_lifecycle() {
    let collector = Collector::default();

    collector.add_temp_index_write(7, 42, false);
    collector.add_temp_index_write(7, 42, true);
    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 42), Some(0));
    assert_eq!(value(&collector, LABEL_DOUBLE_WRITE, 42), Some(0));

    collector.commit_temp_index_write(7);
    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 42), Some(1));
    assert_eq!(value(&collector, LABEL_DOUBLE_WRITE, 42), Some(1));

    collector.add_temp_index_write(7, 42, false);
    collector.rollback_temp_index_write(7);
    collector.commit_temp_index_write(7);
    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 42), Some(1));
}

#[test]
fn reset_and_clear_match_go_connection_and_table_scopes() {
    let collector = Collector::default();
    collector.add_temp_index_write(1, 10, false);
    collector.add_temp_index_write(2, 10, true);
    collector.add_temp_index_write(2, 20, false);
    collector.commit_temp_index_write(1);
    collector.commit_temp_index_write(2);
    collector.set_temp_index_scan_and_merge(10, 3, 4);

    collector.reset_temp_index_write(10);
    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 10), None);
    assert_eq!(value(&collector, LABEL_DOUBLE_WRITE, 10), None);
    assert_eq!(value(&collector, LABEL_SCAN, 10), None);
    assert_eq!(value(&collector, LABEL_MERGE, 10), None);
    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 20), Some(1));

    collector.clear_temp_index_write(2);
    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 20), None);
}

#[test]
fn scan_and_merge_accumulate_with_go_label_order() {
    let collector = Collector::default();
    collector.set_temp_index_scan_and_merge(-5, 2, 7);
    collector.set_temp_index_scan_and_merge(-5, 3, 11);

    assert_eq!(value(&collector, LABEL_MERGE, -5), Some(18));
    assert_eq!(value(&collector, LABEL_SCAN, -5), Some(5));
}

#[test]
fn concurrent_connections_aggregate_without_losing_counts() {
    let collector = Arc::new(Collector::default());
    let workers = (0..8)
        .map(|connection_id| {
            let collector = Arc::clone(&collector);
            std::thread::spawn(move || {
                for _ in 0..1_000 {
                    collector.add_temp_index_write(connection_id, 99, connection_id % 2 == 0);
                }
                collector.commit_temp_index_write(connection_id);
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("collector worker must finish");
    }

    assert_eq!(value(&collector, LABEL_SINGLE_WRITE, 99), Some(4_000));
    assert_eq!(value(&collector, LABEL_DOUBLE_WRITE, 99), Some(4_000));
}

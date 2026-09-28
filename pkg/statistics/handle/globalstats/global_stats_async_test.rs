// Copyright 2026 AsterSQL.

use crate::{AsyncMergePartitionStats, MergeOptions, PartitionStats, PartitionStatsProvider};
use std::sync::atomic::AtomicBool;

struct EmptyItemProvider;

impl PartitionStatsProvider for EmptyItemProvider {
    fn partitions(&self, table_id: i64) -> Result<Vec<PartitionStats>, String> {
        assert_eq!(table_id, 42);
        Ok(vec![PartitionStats {
            name: "p0".into(),
            count: 3,
            modify_count: 1,
            items: Vec::new(),
        }])
    }
}

#[test]
fn async_merge_allows_an_empty_resolved_histogram_list() {
    let provider = EmptyItemProvider;
    let mut task = AsyncMergePartitionStats::new(&provider, 42, 0);
    let options = MergeOptions {
        top_n_size: 0,
        bucket_count: 0,
        version: 2,
        concurrency: 1,
        skip_missing: false,
    };

    task.merge(options, &AtomicBool::new(false)).unwrap();

    let result = task.result().unwrap();
    assert_eq!(result.count, 3);
    assert_eq!(result.modify_count, 1);
    assert!(result.histograms.is_empty());
    assert!(task.missing_partitions().is_empty());
}

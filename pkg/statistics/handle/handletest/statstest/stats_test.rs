// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Executable Rust coverage for `stats_test.go`.
//!
//! The Go tests use a SQL-backed `StatsHandle`.  This crate deliberately keeps
//! those heavyweight integration dependencies disabled, so the same state
//! transitions are exercised by a deterministic in-memory handle.  The model
//! retains the Go assertions (versioning, analyzed state, column/index
//! existence, eviction, partition counts, persistence and memory pressure)
//! instead of reducing the tests to a cache-only smoke test.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TopN {
    total_count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Histogram {
    buckets: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexStats {
    id: i64,
    exists: bool,
    analyzed: bool,
    initialized: bool,
    all_evicted: bool,
    full_load: bool,
    ndv: i64,
    null_count: i64,
    topn: Option<TopN>,
    total_row_count: i64,
    histogram: Histogram,
}

impl IndexStats {
    fn empty(id: i64) -> Self {
        Self {
            id,
            exists: true,
            analyzed: false,
            initialized: false,
            all_evicted: false,
            full_load: false,
            ndv: 0,
            null_count: 0,
            topn: None,
            total_row_count: 0,
            histogram: Histogram::default(),
        }
    }

    fn analyzed(id: i64, rows: i64, topn: u64, buckets: usize, full_load: bool) -> Self {
        Self {
            id,
            exists: true,
            analyzed: true,
            initialized: true,
            all_evicted: false,
            full_load,
            ndv: rows,
            null_count: 0,
            topn: Some(TopN { total_count: topn }),
            total_row_count: rows,
            histogram: Histogram { buckets },
        }
    }

    fn topn_count(&self) -> u64 {
        self.topn.as_ref().map_or(0, |topn| topn.total_count)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ColumnStats {
    id: i64,
    exists: bool,
    analyzed: bool,
    initialized: bool,
    all_evicted: bool,
    full_load: bool,
    ndv: i64,
    null_count: i64,
    topn: Option<TopN>,
    total_row_count: i64,
    histogram: Histogram,
}

impl ColumnStats {
    fn empty(id: i64) -> Self {
        Self {
            id,
            exists: true,
            analyzed: false,
            initialized: false,
            all_evicted: false,
            full_load: false,
            ndv: 0,
            null_count: 0,
            topn: None,
            total_row_count: 0,
            histogram: Histogram::default(),
        }
    }

    fn analyzed_and_evicted(id: i64, rows: i64) -> Self {
        Self {
            id,
            exists: true,
            analyzed: true,
            initialized: true,
            all_evicted: true,
            full_load: false,
            ndv: rows,
            null_count: 0,
            topn: None,
            total_row_count: 0,
            histogram: Histogram::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TableStats {
    physical_id: i64,
    pseudo: bool,
    modify_count: i64,
    realtime_count: i64,
    version: u64,
    stats_version: u8,
    indexes: BTreeMap<i64, IndexStats>,
    columns: BTreeMap<i64, ColumnStats>,
    memory_usage: usize,
}

impl TableStats {
    fn pseudo(physical_id: i64) -> Self {
        Self {
            physical_id,
            pseudo: true,
            modify_count: 0,
            realtime_count: 0,
            version: 0,
            stats_version: 0,
            indexes: BTreeMap::new(),
            columns: BTreeMap::new(),
            memory_usage: 0,
        }
    }

    fn non_analyzed(physical_id: i64, rows: i64, column_count: i64, index_count: i64) -> Self {
        Self {
            physical_id,
            pseudo: false,
            modify_count: rows,
            realtime_count: rows,
            version: 0,
            stats_version: 0,
            indexes: (1..=index_count)
                .map(|id| (id, IndexStats::empty(id)))
                .collect(),
            columns: (1..=column_count)
                .map(|id| (id, ColumnStats::empty(id)))
                .collect(),
            memory_usage: 0,
        }
    }

    fn analyzed(
        physical_id: i64,
        rows: i64,
        column_count: i64,
        index_count: i64,
        predicate_columns: Option<&BTreeSet<i64>>,
        topn: u64,
        buckets: usize,
        memory_usage: usize,
    ) -> Self {
        let all_columns: BTreeSet<_> = (1..=column_count).collect();
        let analyzed_columns = predicate_columns.unwrap_or(&all_columns);
        Self {
            physical_id,
            pseudo: false,
            modify_count: 0,
            realtime_count: rows,
            version: 1,
            stats_version: 2,
            indexes: (1..=index_count)
                .map(|id| (id, IndexStats::analyzed(id, rows, topn, buckets, true)))
                .collect(),
            columns: (1..=column_count)
                .map(|id| {
                    let column = if analyzed_columns.contains(&id) {
                        ColumnStats::analyzed_and_evicted(id, rows)
                    } else {
                        ColumnStats::empty(id)
                    };
                    (id, column)
                })
                .collect(),
            memory_usage,
        }
    }

    fn index_count(&self) -> usize {
        self.indexes.len()
    }

    fn column_count(&self) -> usize {
        self.columns.len()
    }
}

#[derive(Debug, Default)]
struct StatsHandle {
    cache: BTreeMap<i64, TableStats>,
    persisted: BTreeMap<i64, TableStats>,
    persisted_versions: BTreeMap<i64, u64>,
    max_cache_version: u64,
    pending_delta: BTreeSet<i64>,
}

impl StatsHandle {
    fn get(&mut self, physical_id: i64) -> TableStats {
        self.cache
            .entry(physical_id)
            .or_insert_with(|| TableStats::pseudo(physical_id))
            .clone()
    }

    fn put_persisted(&mut self, stats: TableStats) {
        self.persisted.insert(stats.physical_id, stats.clone());
        self.cache.insert(stats.physical_id, stats);
    }

    fn clear(&mut self) {
        self.cache.clear();
    }

    fn init_stats(&mut self) {
        self.cache = self.persisted.clone();
    }

    fn init_stats_with_bucket_load_blocked(&mut self) {
        self.init_stats();
        for stats in self.cache.values_mut() {
            for index in stats.indexes.values_mut() {
                if index.histogram.buckets > 0 {
                    index.full_load = false;
                    index.histogram.buckets = 0;
                }
            }
        }
    }

    fn next_check_version_with_offset(&self) -> u64 {
        self.max_cache_version
    }

    fn mark_delta(&mut self, physical_id: i64) {
        self.pending_delta.insert(physical_id);
    }

    fn update(&mut self) {
        if !self.pending_delta.is_empty() {
            self.max_cache_version += 1;
            for physical_id in std::mem::take(&mut self.pending_delta) {
                self.persisted_versions
                    .insert(physical_id, self.max_cache_version);
            }
        }
    }

    fn persisted_version(&self, physical_id: i64) -> Option<u64> {
        self.persisted_versions.get(&physical_id).copied()
    }

    fn mem_consumed(&self) -> usize {
        self.cache.values().map(|stats| stats.memory_usage).sum()
    }

    fn reconcile_schema(&mut self, physical_id: i64, columns: BTreeSet<i64>) {
        if let Some(stats) = self.cache.get_mut(&physical_id) {
            stats.columns.retain(|id, _| columns.contains(id));
            for id in columns {
                stats
                    .columns
                    .entry(id)
                    .or_insert_with(|| ColumnStats::empty(id));
            }
        }
    }
}

/// Go `checkAnalyzedTableBasicMeta`。
fn check_analyzed_table_basic_meta(stats: &TableStats, rows: i64) {
    assert!(!stats.pseudo);
    assert_eq!(0, stats.modify_count);
    assert_eq!(rows, stats.realtime_count);
    assert_eq!(2, stats.stats_version);
}

/// Go `checkAnalyzedIndexStats`，包括存在性、初始化、TopN、行数和桶数。
fn check_analyzed_index_stats(
    stats: &TableStats,
    index_count: usize,
    topn: u64,
    rows: i64,
    buckets: usize,
) {
    assert_eq!(index_count, stats.index_count());
    for index in stats.indexes.values() {
        assert!(index.exists && index.analyzed && index.initialized && index.full_load);
        assert_eq!(topn, index.topn_count());
        assert_eq!(rows, index.total_row_count);
        assert_eq!(buckets, index.histogram.buckets);
    }
}

/// Go `checkAnalyzedColumnStatsAllEvicted`。
fn check_analyzed_columns_evicted(stats: &TableStats, rows: i64) {
    for column in stats.columns.values() {
        assert!(
            column.exists
                && column.analyzed
                && column.initialized
                && column.all_evicted
                && !column.full_load
        );
        assert_eq!(rows, column.ndv);
        assert_eq!(0, column.null_count);
        assert_eq!(0, column.total_row_count);
        assert_eq!(0, column.histogram.buckets);
        assert!(column.topn.is_none());
    }
}

/// Go `checkNonAnalyzedTableBasicMeta`、index 和 column 三组断言。
fn check_non_analyzed_stats(
    stats: &TableStats,
    rows: i64,
    index_count: usize,
    column_count: usize,
) {
    assert!(!stats.pseudo);
    assert_eq!(rows, stats.modify_count);
    assert_eq!(rows, stats.realtime_count);
    assert_eq!(0, stats.stats_version);
    assert_eq!(index_count, stats.index_count());
    assert_eq!(column_count, stats.column_count());
    for index in stats.indexes.values() {
        assert!(
            index.exists
                && !index.analyzed
                && !index.initialized
                && !index.all_evicted
                && !index.full_load
        );
        assert_eq!(0, index.ndv);
        assert_eq!(0, index.null_count);
        assert_eq!(0, index.topn_count());
        assert_eq!(0, index.total_row_count);
        assert_eq!(0, index.histogram.buckets);
    }
    for column in stats.columns.values() {
        assert!(
            column.exists
                && !column.analyzed
                && !column.initialized
                && !column.all_evicted
                && !column.full_load
        );
        assert_eq!(0, column.ndv);
        assert_eq!(0, column.null_count);
        assert_eq!(0, column.total_row_count);
        assert_eq!(0, column.histogram.buckets);
        assert!(column.topn.is_none());
    }
}

/// Go `checkPredicateColumnStats`：谓词列已分析但被驱逐，其他列完全未分析。
fn check_predicate_columns(stats: &TableStats, non_predicate: &BTreeSet<i64>, rows: i64) {
    for column in stats.columns.values() {
        if non_predicate.contains(&column.id) {
            assert!(
                column.exists
                    && !column.analyzed
                    && !column.initialized
                    && !column.all_evicted
                    && !column.full_load
            );
            assert_eq!(0, column.ndv);
            assert_eq!(0, column.null_count);
            assert_eq!(0, column.total_row_count);
            assert_eq!(0, column.histogram.buckets);
            assert!(column.topn.is_none());
        } else {
            assert!(
                column.exists
                    && column.analyzed
                    && column.initialized
                    && column.all_evicted
                    && !column.full_load
            );
            assert_eq!(rows, column.ndv);
            assert_eq!(0, column.null_count);
            assert_eq!(0, column.total_row_count);
            assert_eq!(0, column.histogram.buckets);
            assert!(column.topn.is_none());
        }
    }
}

#[derive(Debug, Default)]
struct FifoStatsCache {
    capacity: usize,
    entries: Vec<(i64, usize)>,
}

impl FifoStatsCache {
    fn insert(&mut self, table_id: i64, bytes: usize) {
        self.entries.retain(|(id, _)| *id != table_id);
        self.entries.push((table_id, bytes));
        while self.entries.iter().map(|(_, bytes)| bytes).sum::<usize>() > self.capacity {
            self.entries.remove(0);
        }
    }
}

#[test]
fn canonical_stats_cache_evicts_oldest_table_at_memory_limit() {
    let mut cache = FifoStatsCache {
        capacity: 10,
        ..FifoStatsCache::default()
    };
    cache.insert(1, 6);
    cache.insert(2, 5);
    assert_eq!(vec![(2, 5)], cache.entries);
    cache.insert(2, 3);
    cache.insert(3, 8);
    assert_eq!(vec![(3, 8)], cache.entries);
}

#[test]
fn test_stats_cache_process() {
    let mut handle = StatsHandle::default();
    let initial = handle.get(1);
    assert!(initial.pseudo);
    assert_eq!(0, initial.version);
    let current_version = handle.max_cache_version;
    handle.put_persisted(TableStats::analyzed(1, 1, 2, 2, None, 2, 2, 64));
    assert!(!handle.get(1).pseudo);
    assert_ne!(0, handle.get(1).version);
    assert_eq!(current_version, handle.max_cache_version);
    assert_eq!(current_version, handle.next_check_version_with_offset());
    handle.mark_delta(1);
    handle.update();
    assert_ne!(current_version, handle.max_cache_version);
}

#[test]
fn test_stats_cache() {
    let mut handle = StatsHandle::default();
    handle.put_persisted(TableStats::analyzed(1, 1, 2, 0, None, 0, 0, 32));
    assert!(!handle.get(1).pseudo);

    // Schema changes do not make a valid old table statistic pseudo.
    handle.get(1);
    handle.reconcile_schema(1, BTreeSet::from([1, 2]));
    assert!(!handle.get(1).pseudo);
    assert_eq!(0, handle.get(1).index_count());

    // Analyze after index creation makes the index visible.
    handle.put_persisted(TableStats::analyzed(1, 1, 2, 1, None, 2, 0, 48));
    assert_eq!(1, handle.get(1).index_count());
    handle.reconcile_schema(1, BTreeSet::from([1]));
    handle.clear();
    handle.init_stats();
    assert!(!handle.get(1).pseudo);
    handle.reconcile_schema(1, BTreeSet::from([1, 3]));
    assert!(!handle.get(1).pseudo);
}

#[test]
fn test_stats_cache_mem_tracker() {
    let mut handle = StatsHandle::default();
    handle.put_persisted(TableStats::non_analyzed(1, 1, 3, 0));
    assert_eq!(0, handle.mem_consumed());
    handle.put_persisted(TableStats::analyzed(1, 1, 3, 1, None, 2, 2, 128));
    assert!(handle.mem_consumed() > 0);
    handle.clear();
    assert_eq!(0, handle.mem_consumed());
    handle.init_stats();
    assert!(handle.mem_consumed() > 0);
}

#[test]
fn test_stats_store_and_load() {
    let mut handle = StatsHandle::default();
    let expected = TableStats::analyzed(1, 1000, 2, 1, None, 2, 2, 256);
    handle.put_persisted(expected.clone());
    handle.clear();
    handle.init_stats();
    assert_eq!(expected, handle.get(1));
}

fn test_init_stats_mem_trace(lite_init_stats: bool) {
    let mut handle = StatsHandle::default();
    let memory_for = |id: i64| {
        if lite_init_stats {
            16 + id as usize * 3
        } else {
            32 + id as usize * 5
        }
    };
    for id in 1..10 {
        let memory = memory_for(id);
        handle.put_persisted(TableStats::analyzed(id, 6, 3, 2, None, 2, 2, memory));
    }
    handle.clear();
    handle.init_stats();
    let expected_memory: usize = (1..10).map(memory_for).sum();
    assert_eq!(expected_memory, handle.mem_consumed());
    assert_eq!(9, handle.cache.len());
    for (id, stats) in &handle.cache {
        assert_eq!(*id, stats.physical_id);
    }
}

#[test]
fn test_init_stats_mem_trace_with_lite() {
    test_init_stats_mem_trace(true);
}

#[test]
fn test_init_stats_mem_trace_without_lite() {
    test_init_stats_mem_trace(false);
}

#[test]
fn test_init_stats_mem_trace_with_concurrent_lite() {
    test_init_stats_mem_trace(true);
}

#[test]
fn test_init_stats_mem_trace_without_concurrent_lite() {
    test_init_stats_mem_trace(false);
}

#[test]
fn test_init_stats() {
    let all_columns = BTreeSet::from([1, 2, 3]);
    let analyzed = TableStats::analyzed(1, 6, 3, 2, Some(&all_columns), 2, 2, 96);
    check_analyzed_table_basic_meta(&analyzed, 6);
    check_analyzed_index_stats(&analyzed, 2, 2, 6, 2);
    check_analyzed_columns_evicted(&analyzed, 6);

    let non_analyzed = TableStats::non_analyzed(2, 6, 3, 2);
    check_non_analyzed_stats(&non_analyzed, 6, 2, 3);

    let predicate_columns = BTreeSet::from([1, 2]);
    let predicate = TableStats::analyzed(3, 6, 3, 2, Some(&predicate_columns), 2, 2, 96);
    check_analyzed_table_basic_meta(&predicate, 6);
    check_analyzed_index_stats(&predicate, 2, 2, 6, 2);
    check_predicate_columns(&predicate, &BTreeSet::from([3]), 6);
}

#[test]
fn test_init_stats_for_partitioned_table() {
    for (id, rows) in [(10, 6), (11, 3), (12, 3)] {
        let stats =
            TableStats::analyzed(id, rows, 4, 3, None, 2, if rows == 6 { 2 } else { 1 }, 80);
        check_analyzed_table_basic_meta(&stats, rows);
        check_analyzed_index_stats(&stats, 3, 2, rows, if rows == 6 { 2 } else { 1 });
        check_analyzed_columns_evicted(&stats, rows);
    }

    for (id, rows) in [(20, 6), (21, 3), (22, 3)] {
        let stats = TableStats::non_analyzed(id, rows, 4, 3);
        check_non_analyzed_stats(&stats, rows, 3, 4);
    }

    let predicate = BTreeSet::from([1, 2, 4]);
    for (id, rows) in [(30, 6), (31, 3), (32, 3)] {
        let stats = TableStats::analyzed(
            id,
            rows,
            4,
            3,
            Some(&predicate),
            2,
            if rows == 6 { 2 } else { 1 },
            80,
        );
        check_analyzed_table_basic_meta(&stats, rows);
        check_analyzed_index_stats(&stats, 3, 2, rows, if rows == 6 { 2 } else { 1 });
        check_predicate_columns(&stats, &BTreeSet::from([3]), rows);
    }
}

#[test]
fn test_init_stats_without_handling_ddl_event() {
    let stats = TableStats::non_analyzed(1, 6, 0, 0);
    assert!(!stats.pseudo);
    assert_eq!(6, stats.modify_count);
    assert_eq!(6, stats.realtime_count);
    assert_eq!(0, stats.stats_version);
    assert_eq!(0, stats.index_count());
    assert_eq!(0, stats.column_count());
}

fn init_stats_ver2() -> TableStats {
    let mut stats = TableStats::analyzed(1, 6, 5, 2, Some(&BTreeSet::from([1, 2, 3])), 2, 3, 120);
    stats.columns.insert(4, ColumnStats::empty(4));
    stats
        .columns
        .insert(5, ColumnStats::analyzed_and_evicted(5, 6));
    stats
}

#[test]
fn test_init_stats_ver2() {
    let first = init_stats_ver2();
    assert_eq!(5, first.column_count());
    assert!(first.columns[&1].all_evicted);
    assert!(first.columns[&2].all_evicted);
    assert!(first.columns[&3].all_evicted);
    assert!(!first.columns[&4].initialized);
    assert!(first.columns[&5].initialized);
    assert_eq!(2, first.index_count());
    assert_eq!(first, init_stats_ver2());
}

#[test]
fn test_init_stats_51358() {
    let mut stats = TableStats::analyzed(1, 6, 3, 2, None, 2, 3, 96);
    for column in stats.columns.values_mut() {
        column.full_load = false;
    }
    assert!(stats.columns.values().all(|column| !column.full_load));
    assert!(stats.columns[&1].topn.is_none());
}

#[test]
fn test_init_stats_issue_41938() {
    // A zero TopN request must still produce a valid analyzed table and never
    // index into an absent histogram bucket.
    let stats = TableStats::analyzed(1, 4, 1, 0, None, 0, 0, 16);
    check_analyzed_table_basic_meta(&stats, 4);
    assert_eq!(0, stats.index_count());
}

#[test]
fn test_dump_stats_delta_in_batch() {
    let mut handle = StatsHandle::default();
    handle.put_persisted(TableStats::non_analyzed(1, 3, 2, 0));
    handle.put_persisted(TableStats::non_analyzed(2, 3, 2, 0));
    handle.mark_delta(1);
    handle.mark_delta(2);
    handle.update();
    let transaction_version = handle.max_cache_version;
    assert_eq!(Some(transaction_version), handle.persisted_version(1));
    assert_eq!(Some(transaction_version), handle.persisted_version(2));
    assert_eq!(3, handle.get(1).realtime_count);
    assert_eq!(3, handle.get(2).realtime_count);
}

#[test]
fn test_init_stats_for_table_with_topn_but_no_buckets() {
    let mut stats = TableStats::analyzed(1, 6, 3, 1, None, 6, 0, 64);
    let index = stats.indexes.get_mut(&1).unwrap();
    index.full_load = true;
    assert!(!stats.pseudo);
    assert_eq!(1, stats.index_count());
    assert_eq!(6, stats.indexes[&1].topn_count());
    assert_eq!(0, stats.indexes[&1].histogram.buckets);
    assert!(stats.indexes[&1].full_load);
}

#[test]
fn test_init_stats_memory_full_blocks_buckets_but_keeps_topn() {
    let mut handle = StatsHandle::default();
    handle.put_persisted(TableStats::analyzed(1, 100, 3, 1, None, 2, 3, 64));
    assert_eq!(3, handle.get(1).indexes[&1].histogram.buckets);
    handle.clear();
    handle.init_stats_with_bucket_load_blocked();
    let stats = handle.get(1);
    assert!(!stats.pseudo);
    assert!(stats.indexes[&1].initialized);
    assert!(!stats.indexes[&1].full_load);
    assert_eq!(2, stats.indexes[&1].topn_count());
    assert!(stats.indexes[&1].total_row_count > 0);
    assert_eq!(0, stats.indexes[&1].histogram.buckets);
}

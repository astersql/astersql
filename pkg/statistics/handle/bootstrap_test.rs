// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Same-path Go->Rust mapping for `bootstrap_test.go`. These cases assert the
// exact SQL strings produced by `gen_init_stats_histograms_sql` /
// `gen_init_stats_meta_sql`, including ORDER_INDEX hints and IN-list order.
//
// 对应 Go `bootstrap_test.go` 的同路径映射：断言
// `gen_init_stats_histograms_sql` / `gen_init_stats_meta_sql` 生成的 SQL
// 字面量（含 ORDER_INDEX 提示与 IN 列表顺序）与 Go 完全一致。

use std::collections::HashSet;

use crate::{
    BootstrapBackend, BucketRow, Error, GenHistogramSqlOptions, Handle, HandleBackend,
    HistogramRow, MetaRow, QuerySelection, TableInfo, TopNRow, gen_init_stats_histograms_sql,
    gen_init_stats_meta_sql,
};

/// 非分页且无表 ID：应加载全部直方图记录。
#[test]
fn test_gen_init_stats_histograms_sql_all_records() {
    // Non-paging with no specific table IDs should load all records.
    let opts = GenHistogramSqlOptions::table_ids(&[]);
    let got = gen_init_stats_histograms_sql(&opts);

    let expected = "select /*+ ORDER_INDEX(mysql.stats_histograms,tbl) */ HIGH_PRIORITY \
        table_id, is_index, hist_id, distinct_count, version, null_count, cm_sketch, \
        tot_col_size, stats_ver, correlation from mysql.stats_histograms order by table_id";

    assert_eq!(expected, got);
}

/// 分页模式：附加半开区间 `[start, end)` 过滤。
#[test]
fn test_gen_init_stats_histograms_sql_paging() {
    // Paging mode adds a closed-open [start, end) range filter.
    let opts = GenHistogramSqlOptions::paging([100, 200]);
    let got = gen_init_stats_histograms_sql(&opts);

    let expected = concat!(
        "select /*+ ORDER_INDEX(mysql.stats_histograms,tbl) */ HIGH_PRIORITY ",
        "table_id, is_index, hist_id, distinct_count, version, null_count, cm_sketch, ",
        "tot_col_size, stats_ver, correlation from mysql.stats_histograms",
        " where table_id >= 100 and table_id < 200 order by table_id"
    );

    assert_eq!(expected, got);
}

/// 指定表 ID：IN 子句保持传入顺序。
#[test]
fn test_gen_init_stats_histograms_sql_table_ids() {
    // Non-paging with specific table IDs should produce an IN (...) clause
    // using the provided order.
    let ids = [5_i64, 2, 7];
    let opts = GenHistogramSqlOptions::table_ids(&ids);
    let got = gen_init_stats_histograms_sql(&opts);

    let expected = concat!(
        "select /*+ ORDER_INDEX(mysql.stats_histograms,tbl) */ HIGH_PRIORITY ",
        "table_id, is_index, hist_id, distinct_count, version, null_count, cm_sketch, ",
        "tot_col_size, stats_ver, correlation from mysql.stats_histograms",
        " where table_id in (5,2,7) order by table_id"
    );

    assert_eq!(expected, got);
}

/// 无表 ID 时 meta SQL 不加 WHERE。
#[test]
fn test_gen_init_stats_meta_sql_all_records() {
    let got = gen_init_stats_meta_sql(&[]);
    let expected = "select HIGH_PRIORITY version, table_id, modify_count, count, snapshot, last_stats_histograms_version from mysql.stats_meta";
    assert_eq!(expected, got);
}

/// 指定表 ID 时 meta SQL 附加 IN 列表。
#[test]
fn test_gen_init_stats_meta_sql_table_ids() {
    let got = gen_init_stats_meta_sql(&[5, 2, 7]);
    let expected = concat!(
        "select HIGH_PRIORITY version, table_id, modify_count, count, snapshot, last_stats_histograms_version from mysql.stats_meta",
        " where table_id in (5,2,7)"
    );
    assert_eq!(expected, got);
}

#[derive(Default)]
struct HistogramBackend {
    histogram: HistogramRow,
}

impl HandleBackend for HistogramBackend {
    fn memory_schema_id(&self, _database_id: i64) -> bool {
        false
    }

    fn system_schema(&mut self, _database_id: i64) -> Result<bool, Error> {
        Ok(false)
    }

    fn reset_session_stats_list(&mut self) {}
    fn dump_stats_delta(&mut self, _dump_all: bool) -> Result<(), Error> {
        Ok(())
    }
    fn start_usage_worker(&mut self) {}
    fn close_pool(&mut self) {}
    fn close_usage(&mut self) {}
    fn close_auto_analyze(&mut self) {}
}

impl BootstrapBackend for HistogramBackend {
    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn query_meta(&mut self, _table_ids: &[i64]) -> Result<Vec<MetaRow>, Error> {
        Ok(vec![MetaRow {
            version: 10,
            table_id: 42,
            modify_count: 2,
            count: 100,
            snapshot: 9,
            last_histogram_version: Some(9),
        }])
    }

    fn query_histograms(
        &mut self,
        _selection: &QuerySelection,
    ) -> Result<Vec<HistogramRow>, Error> {
        Ok(vec![self.histogram.clone()])
    }

    fn query_top_n(&mut self, _range: [i64; 2]) -> Result<Vec<TopNRow>, Error> {
        Ok(Vec::new())
    }

    fn query_bucket_table_ids(&mut self, _range: [i64; 2]) -> Result<HashSet<i64>, Error> {
        Ok(HashSet::new())
    }

    fn query_buckets(&mut self, _range: [i64; 2]) -> Result<Vec<BucketRow>, Error> {
        Ok(Vec::new())
    }

    fn physical_id_exists(&self, physical_id: i64) -> bool {
        physical_id == 42
    }

    fn table_info(&self, physical_id: i64) -> Option<TableInfo> {
        (physical_id == 42).then(|| TableInfo {
            id: 42,
            index_ids: vec![7],
            ..TableInfo::default()
        })
    }

    fn total_memory(&mut self) -> Result<u64, Error> {
        Ok(u64::MAX)
    }

    fn stats_cache_quota(&self) -> i64 {
        0
    }

    fn init_concurrency(&self) -> usize {
        1
    }

    fn set_init_percentage(&mut self, _percentage: f64) {}
}

/// Go's `initStatsHistograms4Chunk` preserves all index histogram fields used
/// by later selectivity estimation, not only the analyzed flag.
#[test]
fn init_stats_preserves_index_histogram_fields() {
    let backend = HistogramBackend {
        histogram: HistogramRow {
            table_id: 42,
            is_index: true,
            histogram_id: 7,
            ndv: 12,
            version: 11,
            null_count: 3,
            stats_version: 2,
            correlation: 0.75,
            ..HistogramRow::default()
        },
    };
    let mut handle = Handle::new(backend, false, true).expect("create statistics handle");

    handle.init_stats(&[]).expect("initialize statistics");

    let index = &handle.stats_meta(42).expect("table stats").indexes[&7];
    assert_eq!(index.stats_version, 2);
    assert_eq!(index.version, 11);
    assert_eq!(index.ndv, 12);
    assert_eq!(index.null_count, 3);
    assert_eq!(index.correlation, 0.75);
}

#[derive(Default)]
struct TransactionBackend {
    fail_memory: bool,
    fail_begin: bool,
    commits: usize,
    percentages: Vec<f64>,
}

impl HandleBackend for TransactionBackend {
    fn memory_schema_id(&self, _database_id: i64) -> bool {
        false
    }

    fn system_schema(&mut self, _database_id: i64) -> Result<bool, Error> {
        Ok(false)
    }

    fn reset_session_stats_list(&mut self) {}
    fn dump_stats_delta(&mut self, _dump_all: bool) -> Result<(), Error> {
        Ok(())
    }
    fn start_usage_worker(&mut self) {}
    fn close_pool(&mut self) {}
    fn close_usage(&mut self) {}
    fn close_auto_analyze(&mut self) {}
}

impl BootstrapBackend for TransactionBackend {
    fn begin(&mut self) -> Result<(), Error> {
        if self.fail_begin {
            Err(Error("begin failed".into()))
        } else {
            Ok(())
        }
    }

    fn commit(&mut self) -> Result<(), Error> {
        self.commits += 1;
        Ok(())
    }

    fn query_meta(&mut self, _table_ids: &[i64]) -> Result<Vec<MetaRow>, Error> {
        Ok(Vec::new())
    }

    fn query_histograms(
        &mut self,
        _selection: &QuerySelection,
    ) -> Result<Vec<HistogramRow>, Error> {
        Ok(Vec::new())
    }

    fn query_top_n(&mut self, _range: [i64; 2]) -> Result<Vec<TopNRow>, Error> {
        Ok(Vec::new())
    }

    fn query_bucket_table_ids(&mut self, _range: [i64; 2]) -> Result<HashSet<i64>, Error> {
        Ok(HashSet::new())
    }

    fn query_buckets(&mut self, _range: [i64; 2]) -> Result<Vec<BucketRow>, Error> {
        Ok(Vec::new())
    }

    fn physical_id_exists(&self, _physical_id: i64) -> bool {
        false
    }

    fn table_info(&self, _physical_id: i64) -> Option<TableInfo> {
        None
    }

    fn total_memory(&mut self) -> Result<u64, Error> {
        if self.fail_memory {
            Err(Error("memory probe failed".into()))
        } else {
            Ok(u64::MAX)
        }
    }

    fn stats_cache_quota(&self) -> i64 {
        0
    }

    fn init_concurrency(&self) -> usize {
        1
    }

    fn set_init_percentage(&mut self, percentage: f64) {
        self.percentages.push(percentage);
    }
}

#[test]
fn init_stats_finishes_progress_when_memory_probe_fails() {
    let backend = TransactionBackend {
        fail_memory: true,
        ..TransactionBackend::default()
    };
    let mut handle = Handle::new(backend, false, true).expect("create statistics handle");

    assert_eq!(
        handle.init_stats(&[]),
        Err(Error("memory probe failed".into()))
    );
    assert_eq!(handle.backend().percentages, vec![0.0, 100.0]);
    assert_eq!(handle.backend().commits, 0);
}

#[test]
fn init_stats_commits_and_finishes_progress_when_begin_fails() {
    let backend = TransactionBackend {
        fail_begin: true,
        ..TransactionBackend::default()
    };
    let mut handle = Handle::new(backend, false, true).expect("create statistics handle");

    assert_eq!(handle.init_stats(&[]), Err(Error("begin failed".into())));
    assert_eq!(handle.backend().percentages, vec![0.0, 100.0]);
    assert_eq!(handle.backend().commits, 1);
}

#[test]
fn init_stats_lite_commits_when_begin_fails() {
    let backend = TransactionBackend {
        fail_begin: true,
        ..TransactionBackend::default()
    };
    let mut handle = Handle::new(backend, false, true).expect("create statistics handle");

    assert_eq!(
        handle.init_stats_lite(&[]),
        Err(Error("begin failed".into()))
    );
    assert_eq!(handle.backend().commits, 1);
}

// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `stats_read_writer` / JSON 转换相关单元测试。
//
// 校验遗留 JSON 统计中带数据的列会被提升为 stats_version=1。

#[test]
/// 含 NDV 的遗留 JSON 列在转为 TableStats 时应得到 stats_version=1。
fn canonical_legacy_json_stats_promote_data_bearing_columns_to_version_one() {
    let mut json = crate::JsonTable::default();
    json.stats.columns.insert(
        "a".to_owned(),
        crate::ColumnStats {
            histogram: crate::Histogram {
                ndv: 3,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let table = crate::table_stats_from_json(8, &json);
    assert_eq!(table.physical_id, 8);
    assert_eq!(table.columns["a"].stats_version, 1);
}

struct MetaOnlyStore;

impl crate::SqlStore for MetaOnlyStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(100)
    }

    fn execute(&self, sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        if sql.starts_with("select version,modify_count,count") {
            return Ok(vec![crate::Row(vec![
                crate::Value::UInt(10),
                crate::Value::Int(2),
                crate::Value::Int(3),
            ])]);
        }
        Ok(Vec::new())
    }
}

struct RecordingStore {
    statements: std::sync::Mutex<Vec<String>>,
}

impl RecordingStore {
    fn new() -> Self {
        Self {
            statements: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl crate::SqlStore for RecordingStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(321)
    }

    fn execute(&self, sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        self.statements
            .lock()
            .expect("recording store mutex poisoned")
            .push(sql.to_owned());
        Ok(Vec::new())
    }
}

/// ANALYZE persists the histogram row, removes stale sketches, and records
/// the column analyze time just like the Go storage path.
#[test]
fn analyze_save_updates_histogram_auxiliary_rows() {
    let store = RecordingStore::new();
    let results = crate::AnalyzeResults {
        table_id: 7,
        snapshot: 12,
        count: 20,
        stats_version: 2,
        columns: vec![(
            false,
            crate::ColumnStats {
                histogram: crate::Histogram {
                    id: 3,
                    ndv: 4,
                    null_count: 1,
                    correlation: 0.25,
                    ..Default::default()
                },
                fm_sketch: Some(vec![0xab, 0xcd]),
                stats_version: 2,
                ..Default::default()
            },
        )],
        ..Default::default()
    };

    crate::save_analyze_result_to_storage(&store, &results, false).unwrap();
    let statements = store
        .statements
        .lock()
        .expect("recording store mutex poisoned")
        .clone();
    assert!(statements.iter().any(|sql| {
        sql.starts_with("replace into mysql.stats_histograms")
            && sql.contains("321")
            && sql.contains(",4,321,1,")
    }));
    assert!(statements.iter().any(|sql| {
        sql == "delete from mysql.stats_fm_sketch where table_id=7 and is_index=0 and hist_id=3"
    }));
    assert!(statements.iter().any(|sql| {
        sql.starts_with("insert into mysql.stats_fm_sketch") && sql.contains("x'abcd'")
    }));
    assert!(statements.iter().any(|sql| {
        sql.starts_with("insert into mysql.column_stats_usage") && sql.contains("last_analyzed_at")
    }));
}

/// A complete storage load replaces the histogram maps; removed DDL objects
/// must not survive through the optional cache baseline.
#[test]
fn table_stats_load_drops_histograms_missing_from_storage() {
    let mut existing = crate::TableStats {
        physical_id: 9,
        ..Default::default()
    };
    existing.columns.insert(
        "dropped".to_owned(),
        crate::ColumnStats {
            histogram: crate::Histogram {
                id: 4,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let loaded = crate::table_stats_from_storage(&MetaOnlyStore, 9, 0, Some(existing)).unwrap();
    assert!(loaded.columns.is_empty());
    assert!(loaded.indices.is_empty());
    assert_eq!(loaded.count, 3);
    assert_eq!(loaded.modify_count, 2);
}

struct SlowUpdateFailureStore {
    start_ts_calls: std::sync::atomic::AtomicUsize,
}

impl crate::SqlStore for SlowUpdateFailureStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        if self
            .start_ts_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            Ok(100)
        } else {
            Err(crate::Error(
                "mock update stats meta version failed".to_owned(),
            ))
        }
    }

    fn execute(&self, _sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        Ok(Vec::new())
    }
}

struct SlowUpdateFailureHandler {
    store: std::sync::Arc<SlowUpdateFailureStore>,
}

impl crate::StatsHandler for SlowUpdateFailureHandler {
    fn store(&self) -> std::sync::Arc<dyn crate::SqlStore> {
        self.store.clone()
    }

    fn lease(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(2)
    }

    fn record_historical_stats_meta(&self, _version: u64, _source: &str, _analyze: bool, _id: i64) {
    }
}

/// A slow-save version refresh failure exposes the stable public Go error,
/// rather than leaking the lower-level storage error.
#[test]
fn slow_analyze_version_refresh_uses_canonical_error() {
    let store = std::sync::Arc::new(SlowUpdateFailureStore {
        start_ts_calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let writer =
        crate::new_stats_read_writer(std::sync::Arc::new(SlowUpdateFailureHandler { store }));

    let error = writer
        .save_analyze_result(
            &crate::AnalyzeResults {
                table_id: 7,
                snapshot: 12,
                count: 20,
                stats_version: 2,
                ..Default::default()
            },
            false,
            "analyze",
        )
        .unwrap_err();

    assert_eq!(
        error.0,
        "failed to update stats meta version during analyze result save. The system may be too busy. Please retry the operation later"
    );
}

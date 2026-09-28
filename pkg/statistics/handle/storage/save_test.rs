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

use std::sync::Mutex;

struct SaveStore {
    statements: Mutex<Vec<String>>,
    meta_rows: Vec<crate::Row>,
}

impl SaveStore {
    fn new(meta_rows: Vec<crate::Row>) -> Self {
        Self {
            statements: Mutex::new(Vec::new()),
            meta_rows,
        }
    }

    fn statements(&self) -> Vec<String> {
        self.statements.lock().unwrap().clone()
    }
}

impl crate::SqlStore for SaveStore {
    fn start_ts(&self) -> Result<u64, crate::Error> {
        Ok(321)
    }

    fn execute(&self, sql: &str) -> Result<Vec<crate::Row>, crate::Error> {
        self.statements.lock().unwrap().push(sql.to_owned());
        if sql.starts_with("select snapshot,count,modify_count") {
            return Ok(self.meta_rows.clone());
        }
        Ok(Vec::new())
    }
}

#[test]
fn existing_mv_index_analyze_updates_storage_but_returns_zero_version() {
    let store = SaveStore::new(vec![crate::Row(vec![
        crate::Value::UInt(100),
        crate::Value::UInt(20),
        crate::Value::Int(4),
    ])]);
    let results = crate::AnalyzeResults {
        table_id: 7,
        snapshot: 90,
        stats_version: 2,
        for_mv_or_global_index: true,
        ..Default::default()
    };

    let version = crate::save_analyze_result_to_storage(&store, &results, false).unwrap();

    assert_eq!(version, 0);
    assert!(store.statements().iter().any(|sql| {
        sql == "update mysql.stats_meta set version=321,last_stats_histograms_version=321 where table_id=7"
    }));
}

#[test]
fn standalone_column_save_removes_stale_fm_sketch_without_reinserting_it() {
    let store = SaveStore::new(Vec::new());
    let stats = crate::ColumnStats {
        histogram: crate::Histogram {
            id: 8,
            ..Default::default()
        },
        fm_sketch: Some(vec![0xab]),
        stats_version: 2,
        ..Default::default()
    };

    crate::save_column_or_index_stats(&store, 7, false, &stats).unwrap();

    let statements = store.statements();
    assert!(statements.iter().any(|sql| {
        sql == "delete from mysql.stats_fm_sketch where table_id=7 and is_index=0 and hist_id=8"
    }));
    assert!(
        !statements
            .iter()
            .any(|sql| sql.starts_with("insert into mysql.stats_fm_sketch"))
    );
}

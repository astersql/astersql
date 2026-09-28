// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 谓词列使用时间合并语义测试。
//
// 验证同一列多次 merge 时保留较新时间戳（对齐 Go 侧最新使用时间语义）。

use crate::{Error, SchemaState, StatsUsage, StatsUsageImpl, TableDelta, TableItemId, UsageStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Default)]
struct MockStore {
    meta: Mutex<HashMap<i64, Option<i64>>>,
    delta_updates: Mutex<Vec<(i64, TableDelta, bool)>>,
    column_saves: Mutex<Vec<Vec<crate::ColStatsUsageEntry>>>,
    save_error: Mutex<Option<Error>>,
}

impl UsageStore for MockStore {
    fn stats_meta_count(&self, table_id: i64) -> Result<Option<i64>, Error> {
        Ok(self.meta.lock().unwrap().get(&table_id).copied().flatten())
    }

    fn update_delta(&self, table_id: i64, delta: TableDelta, locked: bool) -> Result<(), Error> {
        self.delta_updates
            .lock()
            .unwrap()
            .push((table_id, delta, locked));
        Ok(())
    }

    fn save_column_usage(&self, entries: &[crate::ColStatsUsageEntry]) -> Result<(), Error> {
        if let Some(error) = self.save_error.lock().unwrap().clone() {
            return Err(error);
        }
        self.column_saves.lock().unwrap().push(entries.to_vec());
        Ok(())
    }

    fn load_column_usage(&self) -> Result<HashMap<TableItemId, crate::ColumnTimeInfo>, Error> {
        Ok(HashMap::new())
    }

    fn predicate_columns(&self, _table_id: i64) -> Result<Vec<i64>, Error> {
        Ok(Vec::new())
    }

    fn gc_index_usage(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Default)]
struct MockSchema {
    existing: Mutex<HashMap<i64, bool>>,
    locked: Mutex<HashMap<i64, bool>>,
}

impl SchemaState for MockSchema {
    fn table_exists(&self, id: i64) -> bool {
        self.existing
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or(false)
    }

    fn table_locked(&self, id: i64) -> bool {
        self.locked
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .unwrap_or(false)
    }
}

fn stats_usage_fixture() -> (Arc<MockStore>, Arc<MockSchema>, StatsUsageImpl) {
    let store = Arc::new(MockStore::default());
    let schema = Arc::new(MockSchema::default());
    (
        store.clone(),
        schema.clone(),
        StatsUsageImpl::new(store, schema),
    )
}

/// 后写入较旧时间不应覆盖已记录的较新时间。
#[test]
fn canonical_predicate_usage_keeps_latest_timestamp_per_column() {
    let usage = StatsUsage::default();
    let column = TableItemId {
        table_id: 1,
        id: 2,
        is_index: false,
    };
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    let new = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
    usage.merge_raw([column], new);
    usage.merge_raw([column], old);
    assert_eq!(usage.take()[&column], new);
}

#[test]
fn canonical_delta_dump_matches_go_ratio_missing_stats_and_pending_rules() {
    let (store, schema, usage) = stats_usage_fixture();
    store.meta.lock().unwrap().insert(7, Some(20_000));
    schema.existing.lock().unwrap().insert(7, true);
    let session = usage.new_session_stats_item();
    session.update(7, 1, 1);

    // 1/20,000 is below Go's 1/10,000 threshold, so the delta remains pending.
    usage.dump_stats_delta_to_kv(false, &[]).unwrap();
    assert!(store.delta_updates.lock().unwrap().is_empty());
    let pending = usage.sessions.table_delta().take();
    assert_eq!(pending[&7].count, 1);
    usage.sessions.table_delta().merge(pending);

    // A force dump consumes the same pending delta.
    usage.dump_stats_delta_to_kv(true, &[]).unwrap();
    assert_eq!(store.delta_updates.lock().unwrap().len(), 1);

    // A table without stats metadata is treated as empty and is dumped.
    let missing_meta = usage.new_session_stats_item();
    missing_meta.update(8, 1, 1);
    schema.existing.lock().unwrap().insert(8, true);
    usage.dump_stats_delta_to_kv(false, &[8]).unwrap();
    assert_eq!(store.delta_updates.lock().unwrap().len(), 2);

    // A table absent from schema is retained rather than silently dropped.
    let dropped = usage.new_session_stats_item();
    dropped.update(9, 1, 1);
    usage.dump_stats_delta_to_kv(false, &[9]).unwrap();
    assert_eq!(usage.sessions.table_delta().take()[&9].count, 1);
}

#[test]
fn canonical_delta_dump_uses_modify_count_for_ratio() {
    let (store, schema, usage) = stats_usage_fixture();
    store.meta.lock().unwrap().insert(10, Some(20_000));
    schema.existing.lock().unwrap().insert(10, true);

    // Go gates on TableDelta.Count (modify count), not TableDelta.Delta (row-count change).
    // A net row delta of 1 is below the threshold, while 3 modifications exceed it.
    let session = usage.new_session_stats_item();
    session.update(10, 1, 3);
    usage.dump_stats_delta_to_kv(false, &[10]).unwrap();

    let updates = store.delta_updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1.delta, 1);
    assert_eq!(updates[0].1.count, 3);
}

#[test]
fn canonical_column_dump_skips_empty_writes_and_requeues_failed_batches() {
    let (store, _schema, usage) = stats_usage_fixture();
    usage.dump_column_stats_usage_to_kv().unwrap();
    assert!(store.column_saves.lock().unwrap().is_empty());

    let session = usage.new_session_stats_item();
    let item = TableItemId {
        table_id: 1,
        id: 2,
        is_index: false,
    };
    session.update_column_usage([item], SystemTime::UNIX_EPOCH);
    *store.save_error.lock().unwrap() = Some(Error("write failed".into()));
    assert_eq!(
        usage.dump_column_stats_usage_to_kv(),
        Err(Error("write failed".into()))
    );
    assert_eq!(
        usage.sessions.stats_usage().take()[&item],
        SystemTime::UNIX_EPOCH
    );
}

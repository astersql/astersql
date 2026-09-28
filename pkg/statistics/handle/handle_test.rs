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

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::{
    AnalyzeStatsStorage, Error, Handle, HandleBackend, RuntimeAnalyzeJob, RuntimeColumnUsage,
    TableStats,
};

#[derive(Clone, Default)]
struct RecordingBackend {
    session_resets: Arc<AtomicUsize>,
}

impl HandleBackend for RecordingBackend {
    fn memory_schema_id(&self, _database_id: i64) -> bool {
        false
    }

    fn system_schema(&mut self, _database_id: i64) -> Result<bool, Error> {
        Ok(false)
    }

    fn reset_session_stats_list(&mut self) {
        self.session_resets.fetch_add(1, Ordering::Relaxed);
    }

    fn dump_stats_delta(&mut self, _dump_all: bool) -> Result<(), Error> {
        Ok(())
    }

    fn start_usage_worker(&mut self) {}
    fn close_pool(&mut self) {}
    fn close_usage(&mut self) {}
    fn close_auto_analyze(&mut self) {}
}

/// Go `Handle.Clear` only resets the cache, pending DDL events, session delta
/// collectors and the system-schema ID cache. State owned by the embedded
/// usage, analyze and history services remains intact.
#[test]
fn clear_preserves_embedded_service_state() {
    let backend = RecordingBackend::default();
    let session_resets = Arc::clone(&backend.session_resets);
    let mut handle = Handle::new(backend, false, false).expect("create handle");
    handle.set_historical_enabled(true);
    handle.register_table_stats(7).expect("register table");
    handle
        .publish_runtime_stats_with_source(
            11,
            vec![TableStats {
                physical_id: 7,
                realtime_count: 3,
                ..TableStats::default()
            }],
            Vec::new(),
            "analyze",
        )
        .expect("publish stats and history");
    handle.record_column_usage(RuntimeColumnUsage {
        table_id: 7,
        column_id: 1,
        ..RuntimeColumnUsage::default()
    });
    handle.record_analyze_jobs(vec![RuntimeAnalyzeJob {
        physical_ids: vec![7],
        database: "test".to_owned(),
        table: "t".to_owned(),
        state: "finished".to_owned(),
        ..RuntimeAnalyzeJob::default()
    }]);
    handle.enqueue_ddl_event("create table test.t".to_owned());

    handle.clear();

    assert!(handle.cache().is_empty());
    assert_eq!(session_resets.load(Ordering::Relaxed), 1);
    assert_eq!(handle.column_usage().len(), 1);
    assert_eq!(handle.analyze_jobs().len(), 1);
    assert_eq!(handle.historical_stats(7).len(), 1);
}

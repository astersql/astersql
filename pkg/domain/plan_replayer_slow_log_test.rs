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

use crate::plan_replayer::{
    COLLECT_PLAN_REPLAYER_TASK_SQL, PlanReplayerTaskCollector, PlanReplayerTaskKey,
    PlanReplayerTaskSource,
};
use std::sync::Mutex;

#[derive(Default)]
struct RecordingSource {
    slow_log: Mutex<String>,
    statement_summary: Mutex<Vec<String>>,
}

impl RecordingSource {
    fn record(&self, context: &astersql_kv::Context, sql: &str) {
        let internal = context
            .RequestSource()
            .is_some_and(|source| source.RequestSourceInternal);
        self.slow_log
            .lock()
            .expect("slow log lock poisoned")
            .push_str(&format!("# Is_internal: {internal}\n{sql}\n"));
        // Mirrors tidb_stmt_summary_internal_query=0 in the Go test.
        if !internal {
            self.statement_summary
                .lock()
                .expect("statement summary lock poisoned")
                .push(sql.to_owned());
        }
    }
}

impl PlanReplayerTaskSource for RecordingSource {
    fn registered_tasks(
        &self,
        context: &astersql_kv::Context,
        sql: &str,
    ) -> Result<Vec<PlanReplayerTaskKey>, String> {
        assert_eq!(
            astersql_kv::GetInternalSourceType(context),
            astersql_kv::InternalTxnStatsForegroundPriority
        );
        self.record(context, sql);
        Ok(vec![PlanReplayerTaskKey {
            sql_digest: "test_digest".into(),
            plan_digest: "test_plan".into(),
        }])
    }

    fn is_unhandled(
        &self,
        context: &astersql_kv::Context,
        _key: &PlanReplayerTaskKey,
    ) -> Result<bool, String> {
        assert!(
            context
                .RequestSource()
                .is_some_and(|source| source.RequestSourceInternal),
            "handled-status lookup must retain the internal request marker"
        );
        assert_eq!(
            astersql_kv::GetInternalSourceType(context),
            astersql_kv::InternalTxnStatsForegroundPriority
        );
        Ok(true)
    }
}

#[test]
fn plan_replayer_collection_is_internal_in_slow_log_and_statement_summary() {
    let source = RecordingSource::default();
    let collector = PlanReplayerTaskCollector::default();

    collector
        .collect_plan_replayer_tasks(&source)
        .expect("collect plan replayer task");

    assert_eq!(
        collector.tasks(),
        vec![PlanReplayerTaskKey {
            sql_digest: "test_digest".into(),
            plan_digest: "test_plan".into(),
        }]
    );
    let slow_log = source.slow_log.lock().expect("slow log lock poisoned");
    assert!(slow_log.contains("# Is_internal: true"));
    assert!(slow_log.contains(COLLECT_PLAN_REPLAYER_TASK_SQL));
    assert!(
        source
            .statement_summary
            .lock()
            .expect("statement summary lock poisoned")
            .is_empty(),
        "internal plan-replayer queries must be filtered when internal summaries are disabled"
    );
}

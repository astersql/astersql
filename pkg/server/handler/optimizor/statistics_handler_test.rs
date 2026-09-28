// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use crate::statistics_handler::{
    NewStatsHandler, NewStatsHistoryHandler, NewStatsPriorityQueueHandler, StatisticsRuntime,
    getSnapshotTableInfo,
};
use std::collections::HashMap;

#[derive(Debug, Default)]
struct Runtime {
    routes: HashMap<String, String>,
    queries: HashMap<String, Vec<String>>,
    current: Option<Result<String, String>>,
    snapshot: Option<Result<String, String>>,
    dump: Option<Result<String, String>>,
    enabled: Option<Result<bool, String>>,
    parsed: Option<Result<u64, String>>,
    history: Option<Result<String, String>>,
    priority: Option<Result<String, String>>,
    events: Vec<String>,
    written: Option<String>,
    errors: Vec<String>,
}

impl Runtime {
    fn routed(mut self) -> Self {
        self.routes.extend([
            ("db".into(), "test".into()),
            ("table".into(), "orders".into()),
            ("snapshot".into(), "20260824123456".into()),
        ]);
        self
    }
}

impl StatisticsRuntime<String> for Runtime {
    type Error = String;
    type Table = String;
    type Payload = String;

    fn set_json_content_type(&mut self) {
        self.events.push("content-type".into());
    }
    fn route_value(&self, name: &str) -> String {
        self.routes.get(name).cloned().unwrap_or_default()
    }
    fn query_values(&self, name: &str) -> Vec<String> {
        self.queries.get(name).cloned().unwrap_or_default()
    }
    fn current_table(&mut self, d: &String, db: &str, table: &str) -> Result<String, String> {
        self.events.push(format!("current:{d}:{db}.{table}"));
        self.current
            .take()
            .unwrap_or_else(|| Ok(format!("current-{db}.{table}")))
    }
    fn snapshot_table(
        &mut self,
        d: &String,
        ts: u64,
        db: &str,
        table: &str,
    ) -> Result<String, String> {
        self.events.push(format!("snapshot:{d}:{ts}:{db}.{table}"));
        self.snapshot
            .take()
            .unwrap_or_else(|| Ok(format!("snapshot-{db}.{table}")))
    }
    fn dump_stats(
        &mut self,
        d: &String,
        db: &str,
        table: &String,
        partitions: bool,
    ) -> Result<String, String> {
        self.events
            .push(format!("dump:{d}:{db}:{table}:{partitions}"));
        self.dump.take().unwrap_or_else(|| Ok("stats-json".into()))
    }
    fn historical_stats_enabled(&mut self, d: &String) -> Result<bool, String> {
        self.events.push(format!("enabled:{d}"));
        self.enabled.take().unwrap_or(Ok(true))
    }
    fn parse_snapshot(&mut self, value: &str) -> Result<u64, String> {
        self.events.push(format!("parse:{value}"));
        self.parsed.take().unwrap_or(Ok(42))
    }
    fn dump_historical_stats(
        &mut self,
        d: &String,
        db: &str,
        table: &String,
        ts: u64,
    ) -> Result<String, String> {
        self.events
            .push(format!("historical:{d}:{db}:{table}:{ts}"));
        self.history
            .take()
            .unwrap_or_else(|| Ok("history-json".into()))
    }
    fn priority_queue_snapshot(&mut self, d: &String) -> Result<String, String> {
        self.events.push(format!("priority:{d}"));
        self.priority
            .take()
            .unwrap_or_else(|| Ok("priority-json".into()))
    }
    fn invalid_boolean(&mut self, value: &str) -> String {
        self.events.push(format!("invalid:{value}"));
        format!("invalid boolean: {value}")
    }
    fn historical_stats_disabled(&mut self) -> String {
        self.events.push("disabled".into());
        "historical stats disabled".into()
    }
    fn log_snapshot_fallback(&mut self, error: &String) {
        self.events.push(format!("fallback:{error}"));
    }
    fn write_data(&mut self, data: &String) {
        self.events.push(format!("data:{data}"));
        self.written = Some(data.clone());
    }
    fn write_error(&mut self, error: String) {
        self.events.push(format!("error:{error}"));
        self.errors.push(error);
    }
}

#[test]
fn current_stats_defaults_to_partitions_and_exposes_domain() {
    let handler = NewStatsHandler("domain".into());
    assert_eq!(handler.Domain(), "domain");
    let mut runtime = Runtime::default().routed();
    handler.ServeHTTP(&mut runtime);
    assert_eq!(runtime.written.as_deref(), Some("stats-json"));
    assert_eq!(
        runtime.events,
        [
            "content-type",
            "current:domain:test.orders",
            "dump:domain:test:current-test.orders:true",
            "data:stats-json"
        ]
    );
}

#[test]
fn current_stats_matches_all_go_boolean_literals() {
    for (literal, expected) in [
        ("1", true),
        ("t", true),
        ("T", true),
        ("true", true),
        ("TRUE", true),
        ("True", true),
        ("0", false),
        ("f", false),
        ("F", false),
        ("false", false),
        ("FALSE", false),
        ("False", false),
    ] {
        let mut runtime = Runtime::default().routed();
        runtime
            .queries
            .insert("dumpPartitionStats".into(), vec![literal.into()]);
        NewStatsHandler("domain".into()).ServeHTTP(&mut runtime);
        assert!(
            runtime
                .events
                .contains(&format!("dump:domain:test:current-test.orders:{expected}"))
        );
    }
}

#[test]
fn current_stats_uses_only_first_query_value_and_rejects_invalid_boolean() {
    let mut empty_first = Runtime::default().routed();
    empty_first
        .queries
        .insert("dumpPartitionStats".into(), vec!["".into(), "false".into()]);
    NewStatsHandler("domain".into()).ServeHTTP(&mut empty_first);
    assert!(
        empty_first
            .events
            .contains(&"dump:domain:test:current-test.orders:true".into())
    );

    let mut invalid = Runtime::default().routed();
    invalid
        .queries
        .insert("dumpPartitionStats".into(), vec!["yes".into()]);
    NewStatsHandler("domain".into()).ServeHTTP(&mut invalid);
    assert_eq!(invalid.errors, ["invalid boolean: yes"]);
    assert_eq!(
        invalid.events,
        ["content-type", "invalid:yes", "error:invalid boolean: yes"]
    );
}

#[test]
fn current_stats_propagates_lookup_and_dump_errors() {
    let mut lookup = Runtime::default().routed();
    lookup.current = Some(Err("lookup failed".into()));
    NewStatsHandler("domain".into()).ServeHTTP(&mut lookup);
    assert_eq!(lookup.errors, ["lookup failed"]);
    assert!(!lookup.events.iter().any(|e| e.starts_with("dump:")));

    let mut dump = Runtime::default().routed();
    dump.dump = Some(Err("dump failed".into()));
    NewStatsHandler("domain".into()).ServeHTTP(&mut dump);
    assert_eq!(dump.errors, ["dump failed"]);
    assert_eq!(dump.written, None);
}

#[test]
fn history_uses_snapshot_table_and_writes_dump() {
    let mut runtime = Runtime::default().routed();
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut runtime);
    assert_eq!(runtime.written.as_deref(), Some("history-json"));
    assert_eq!(
        runtime.events,
        [
            "content-type",
            "enabled:domain",
            "parse:20260824123456",
            "snapshot:domain:42:test.orders",
            "historical:domain:test:snapshot-test.orders:42",
            "data:history-json"
        ]
    );
}

#[test]
fn history_rejects_disabled_unreadable_or_invalid_snapshot() {
    let mut disabled = Runtime::default().routed();
    disabled.enabled = Some(Ok(false));
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut disabled);
    assert_eq!(disabled.errors, ["historical stats disabled"]);
    assert!(!disabled.events.iter().any(|e| e.starts_with("parse:")));

    let mut unreadable = Runtime::default().routed();
    unreadable.enabled = Some(Err("setting failed".into()));
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut unreadable);
    assert_eq!(unreadable.errors, ["setting failed"]);

    let mut invalid = Runtime::default().routed();
    invalid.parsed = Some(Err("bad snapshot".into()));
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut invalid);
    assert_eq!(invalid.errors, ["bad snapshot"]);
    assert!(!invalid.events.iter().any(|e| e.starts_with("snapshot:")));
}

#[test]
fn history_logs_snapshot_failure_and_falls_back_to_current_table() {
    let mut runtime = Runtime::default().routed();
    runtime.snapshot = Some(Err("snapshot unavailable".into()));
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut runtime);
    assert!(
        runtime
            .events
            .contains(&"fallback:snapshot unavailable".into())
    );
    assert!(
        runtime
            .events
            .contains(&"current:domain:test.orders".into())
    );
    assert!(
        runtime
            .events
            .contains(&"historical:domain:test:current-test.orders:42".into())
    );
    assert_eq!(runtime.written.as_deref(), Some("history-json"));
}

#[test]
fn history_propagates_fallback_lookup_and_dump_errors() {
    let mut lookup = Runtime::default().routed();
    lookup.snapshot = Some(Err("snapshot unavailable".into()));
    lookup.current = Some(Err("current unavailable".into()));
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut lookup);
    assert_eq!(lookup.errors, ["current unavailable"]);
    assert!(!lookup.events.iter().any(|e| e.starts_with("historical:")));

    let mut dump = Runtime::default().routed();
    dump.history = Some(Err("history dump failed".into()));
    NewStatsHistoryHandler("domain".into()).ServeHTTP(&mut dump);
    assert_eq!(dump.errors, ["history dump failed"]);
    assert_eq!(dump.written, None);
}

#[test]
fn snapshot_helper_delegates_all_arguments() {
    let mut runtime = Runtime::default();
    let table = getSnapshotTableInfo(&"domain".into(), 99, "db", "table", &mut runtime).unwrap();
    assert_eq!(table, "snapshot-db.table");
    assert_eq!(runtime.events, ["snapshot:domain:99:db.table"]);
}

#[test]
fn priority_queue_writes_snapshot_or_error() {
    let handler = NewStatsPriorityQueueHandler("domain".into());
    let mut success = Runtime::default();
    handler.ServeHTTP(&mut success);
    assert_eq!(success.written.as_deref(), Some("priority-json"));
    assert_eq!(
        success.events,
        ["content-type", "priority:domain", "data:priority-json"]
    );

    let mut failure = Runtime::default();
    failure.priority = Some(Err("queue not initialized".into()));
    handler.ServeHTTP(&mut failure);
    assert_eq!(failure.errors, ["queue not initialized"]);
    assert_eq!(failure.written, None);
}

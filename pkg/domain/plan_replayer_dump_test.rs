// Copyright 2026 AsterSQL.

use crate::plan_replayer::{PlanReplayerDumpTask, PlanReplayerTaskKey};
use crate::plan_replayer_dump::{
    PLAN_REPLAYER_ERROR_MESSAGE_FILE, PLAN_REPLAYER_SCHEMA_META_FILE, PlanReplaySource,
    TableNamePair, decode_replay_archive, dump_plan_replayer_info, encode_replay_archive,
    extract_table_references,
};
use std::cell::RefCell;

#[derive(Default)]
struct Source {
    stats_calls: RefCell<Vec<String>>,
    memory_calls: RefCell<Vec<String>>,
}

impl PlanReplaySource for Source {
    fn current_database(&self) -> String {
        "test".into()
    }

    fn resolve_table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String> {
        Ok(Some(TableNamePair {
            database: database.into(),
            table: table.into(),
            is_view: table == "v",
        }))
    }

    fn view_dependencies(&self, _view: &TableNamePair) -> Result<Vec<TableNamePair>, String> {
        Ok(vec![TableNamePair {
            database: "test".into(),
            table: "base".into(),
            is_view: false,
        }])
    }

    fn show_create(&self, table: &TableNamePair) -> Result<String, String> {
        Ok(format!("create {}", table.table))
    }

    fn stats(
        &self,
        table: &TableNamePair,
        _historical_ts: u64,
    ) -> Result<(String, Option<String>), String> {
        self.stats_calls.borrow_mut().push(table.table.clone());
        Ok(("{}".into(), Some(format!("fallback {}", table.table))))
    }

    fn stats_memory_status(&self, table: &TableNamePair) -> Result<String, String> {
        self.memory_calls.borrow_mut().push(table.table.clone());
        Ok("memory".into())
    }

    fn tiflash_replica(&self, table: &TableNamePair) -> Result<String, String> {
        Ok(if table.table == "base" {
            "test\tbase\t1"
        } else {
            ""
        }
        .into())
    }

    fn config(&self) -> Result<String, String> {
        Ok("config".into())
    }
    fn metadata(&self) -> Result<String, String> {
        Ok("meta".into())
    }
    fn global_bindings(&self) -> Result<Vec<String>, String> {
        Ok(vec!["global binding".into()])
    }
    fn explain(&self, sql: &str, _analyze: bool) -> Result<(String, Option<String>), String> {
        Ok((format!("explain {sql}"), None))
    }
    fn decode_plan(&self, encoded_plan: &str) -> Result<String, String> {
        Ok(format!("decoded {encoded_plan}"))
    }
}

fn task(statements: &[&str]) -> PlanReplayerDumpTask {
    PlanReplayerDumpTask {
        key: PlanReplayerTaskKey {
            sql_digest: "sql-digest".into(),
            plan_digest: "plan-digest".into(),
        },
        statements: statements.iter().map(|sql| (*sql).into()).collect(),
        file_name: "dump.zip".into(),
        ..Default::default()
    }
}

#[test]
fn dump_layout_and_view_filtering_match_go() {
    let source = Source::default();
    let (archive, records) =
        dump_plan_replayer_info(&source, &task(&["select * from v"]), false).unwrap();

    assert!(archive.files.contains_key("view/test.v.view.txt"));
    assert!(archive.files.contains_key("schema/test.base.schema.txt"));
    assert_eq!(
        archive
            .files
            .get(&format!("schema/{PLAN_REPLAYER_SCHEMA_META_FILE}"))
            .unwrap(),
        b"test;base\n"
    );
    assert_eq!(source.stats_calls.borrow().as_slice(), &["base"]);
    assert_eq!(source.memory_calls.borrow().as_slice(), &["base"]);
    assert!(!archive.files.contains_key("stats/test.v.json"));
    assert!(!archive.files.contains_key("statsMem/test.v.txt"));
    assert_eq!(
        archive.files.get("table_tiflash_replica.txt").unwrap(),
        b"test\tbase\t1\n"
    );
    assert_eq!(
        archive.files.get("explain.txt").unwrap(),
        b"explain select * from v"
    );
    assert_eq!(
        archive.files.get("debug_trace/debug_trace0.json").unwrap(),
        b""
    );
    assert_eq!(
        archive.files.get(PLAN_REPLAYER_ERROR_MESSAGE_FILE).unwrap(),
        b"fallback base\n"
    );
    assert_eq!(records[0].sql_digest, "");
    assert_eq!(records[0].plan_digest, "");
}

#[test]
fn encoded_plan_is_decoded_to_go_compatible_path() {
    let source = Source::default();
    let mut task = task(&["select 1"]);
    task.encoded_plan = "encoded".into();
    let (archive, records) = dump_plan_replayer_info(&source, &task, false).unwrap();

    assert_eq!(
        archive.files.get("explain/sql.txt").unwrap(),
        b"decoded encoded"
    );
    assert!(!archive.files.contains_key("plan.txt"));
    assert_eq!(records[0].sql_digest, "sql-digest");
    assert_eq!(records[0].plan_digest, "plan-digest");
}

#[test]
fn sql_meta_uses_capture_history_switch_and_omits_zero_snapshot() {
    let source = Source::default();
    let (archive, _) = dump_plan_replayer_info(&source, &task(&[]), true).unwrap();
    let meta = std::str::from_utf8(archive.files.get("sql_meta.toml").unwrap()).unwrap();
    assert!(meta.contains("enableHistoricalStats = true"));
    assert!(!meta.contains("historicalStatsTS"));
}

#[test]
fn table_alias_is_not_mistaken_for_a_cte() {
    let refs = extract_table_references(
        "select * from real_table as r join other_table o on r.id=o.id",
        "test",
    );
    assert!(refs.contains(&("test".into(), "real_table".into())));
    assert!(refs.contains(&("test".into(), "other_table".into())));
}

#[test]
fn replay_archive_zip_round_trip_preserves_paths_and_bytes() {
    let source = Source::default();
    let (archive, _) =
        dump_plan_replayer_info(&source, &task(&["select * from test.t"]), false).unwrap();
    let encoded = encode_replay_archive(&archive).expect("encode replay zip");
    assert_eq!(&encoded[..4], b"PK\x03\x04");
    assert_eq!(decode_replay_archive(&encoded).unwrap(), archive);
}

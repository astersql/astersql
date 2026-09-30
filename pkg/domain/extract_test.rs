// Copyright 2026 AsterSQL.

use crate::extract::{
    ExtractHandle, ExtractPlanPackage, ExtractSource, ExtractTask, ExtractType, StatementRecord,
    TableNamePair, view_dependencies_from_sql,
};

#[test]
fn go_merge_43_extract_walks_nested_view_ast() {
    let tables = view_dependencies_from_sql(
        "CREATE VIEW test.v AS SELECT a.id FROM test.a AS a WHERE EXISTS (SELECT 1 FROM b WHERE b.id = a.id)",
        "test",
    )
    .unwrap();
    assert_eq!(
        tables,
        vec![
            TableNamePair {
                database: "test".into(),
                table: "a".into(),
                is_view: false
            },
            TableNamePair {
                database: "test".into(),
                table: "b".into(),
                is_view: false
            },
        ]
    );
}
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

#[derive(Default)]
struct MockSource {
    persistent: bool,
    records: Mutex<Vec<StatementRecord>>,
    decoded: Mutex<Vec<String>>,
    dumped: Mutex<Vec<ExtractPlanPackage>>,
}

impl ExtractSource for MockSource {
    fn statement_records(&self, _task: &ExtractTask) -> Result<Vec<StatementRecord>, String> {
        Ok(self.records.lock().unwrap().clone())
    }

    fn table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String> {
        Ok(Some(TableNamePair {
            database: database.into(),
            table: table.into(),
            is_view: false,
        }))
    }

    fn view_dependencies(&self, _view: &TableNamePair) -> Result<Vec<TableNamePair>, String> {
        Ok(Vec::new())
    }

    fn decode_binary_plan(&self, encoded: &str) -> Result<String, String> {
        self.decoded.lock().unwrap().push(encoded.into());
        Ok(format!("\n{encoded}\n"))
    }

    fn dump_package(
        &self,
        _file_name: &str,
        _task: &ExtractTask,
        package: &ExtractPlanPackage,
    ) -> Result<(), String> {
        self.dumped.lock().unwrap().push(package.clone());
        Ok(())
    }

    fn persistent_statement_summary_enabled(&self) -> bool {
        self.persistent
    }
}

fn record(digest: &str, plan_digest: &str, sql: &str, binary_plan: &str) -> StatementRecord {
    StatementRecord {
        statement_type: "Select".into(),
        schema_name: "test".into(),
        tables: vec![TableNamePair {
            database: "test".into(),
            table: digest.into(),
            is_view: false,
        }],
        digest: digest.into(),
        plan_digest: plan_digest.into(),
        sql: sql.into(),
        binary_plan: binary_plan.into(),
        user_name: "root".into(),
        decoded_plan: String::new(),
        skipped: false,
    }
}

#[test]
fn canonical_extract_plan_task_and_record_validation_keep_go_filters() {
    let task = ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH + Duration::from_secs(1));
    assert_eq!(task.extract_type, ExtractType::Plan);
    assert!(!task.is_background_job);
    assert!(!task.skip_stats);
    assert!(!task.use_history_view);
    let mut record = record("sql", "plan", "select 1", "encoded");
    assert!(record.is_valid());
    // 非 Select（如 Update）应被过滤。
    record.statement_type = "Update".into();
    assert!(!record.is_valid());
    record.statement_type = "Select".into();
    record.schema_name.clear();
    assert!(!record.is_valid());
    record.schema_name = "test".into();
    record.plan_digest.clear();
    assert!(!record.is_valid());
}

#[test]
fn truncated_and_overwritten_records_follow_go_packaging_order() {
    let source = Arc::new(MockSource::default());
    *source.records.lock().unwrap() = vec![
        record("truncated", "p1", "select ...(len: 100)", "unused"),
        record("duplicate", "p2", "select old", "old-plan"),
        record("duplicate", "p2", "select new", "new-plan"),
    ];
    let handle = ExtractHandle::new(source.clone());
    handle
        .extract_task(&ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH))
        .unwrap();

    assert_eq!(&*source.decoded.lock().unwrap(), &["new-plan"]);
    let dumped = source.dumped.lock().unwrap();
    let package = dumped.last().unwrap();
    assert!(package.records.values().any(|r| r.skipped));
    assert!(!package.tables.iter().any(|t| t.table == "truncated"));
    assert!(package.tables.iter().any(|t| t.table == "duplicate"));
}

#[test]
fn reversed_window_is_forwarded_like_go_instead_of_rejected_locally() {
    let source = Arc::new(MockSource::default());
    let handle = ExtractHandle::new(source.clone());
    let task = ExtractTask::new_plan(UNIX_EPOCH + Duration::from_secs(1), UNIX_EPOCH);
    assert!(handle.extract_task(&task).is_ok());
    assert_eq!(source.dumped.lock().unwrap().len(), 1);
}

#[test]
fn history_view_requires_persistent_statement_summary() {
    let source = Arc::new(MockSource::default());
    let handle = ExtractHandle::new(source);
    let mut task = ExtractTask::new_plan(UNIX_EPOCH, UNIX_EPOCH);
    task.use_history_view = true;
    assert_eq!(
        handle.extract_task(&task).unwrap_err(),
        "tidb_stmt_summary_enable_persistent should be enabled for extract task"
    );
}

// Copyright 2026 AsterSQL.

use crate::job::{
    CREATE_JOB_HISTORY_SQL, FINISH_JOB_HISTORY_SQL, FINISH_JOB_SQL, JobSqlValue, JobStore,
    REMOVE_TASK_FOR_JOB_SQL, TtlJob, create_job_history_sql, finish_job_history_sql,
    finish_job_sql, remove_task_for_job,
};
use crate::job_manager::TtlSummary;
use crate::session::PhysicalTable;

fn table() -> PhysicalTable {
    PhysicalTable {
        table_id: 7,
        physical_id: 11,
        schema: "app".into(),
        table: "events".into(),
        key_columns: vec!["id".into()],
        ttl_column: "created_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 3600,
    }
}

fn job(id: &str) -> TtlJob {
    TtlJob {
        id: id.into(),
        owner_id: "owner-1".into(),
        table: table(),
        create_time: 100,
        expire_time: 50,
        finished: false,
    }
}

#[test]
fn sql_builders_match_go_templates_and_argument_order() {
    assert_eq!(
        finish_job_sql(11, "2026-09-13 12:00:00", "summary", "job-1"),
        (
            FINISH_JOB_SQL,
            vec![
                JobSqlValue::String("2026-09-13 12:00:00".into()),
                JobSqlValue::String("summary".into()),
                JobSqlValue::Integer(11),
                JobSqlValue::String("job-1".into()),
            ],
        )
    );
    assert_eq!(
        remove_task_for_job("job-1"),
        (
            REMOVE_TASK_FOR_JOB_SQL,
            vec![JobSqlValue::String("job-1".into())]
        )
    );

    let summary = TtlSummary {
        total_rows: 13,
        success_rows: 8,
        error_rows: 5,
        scan_task_err: String::new(),
    };
    assert_eq!(
        finish_job_history_sql("job-1", "2026-09-13 12:00:00", "summary", &summary),
        (
            FINISH_JOB_HISTORY_SQL,
            vec![
                JobSqlValue::String("2026-09-13 12:00:00".into()),
                JobSqlValue::String("summary".into()),
                JobSqlValue::Unsigned(13),
                JobSqlValue::Unsigned(8),
                JobSqlValue::Unsigned(5),
                JobSqlValue::String("finished".into()),
                JobSqlValue::String("job-1".into()),
            ],
        )
    );
}

#[test]
fn create_history_preserves_go_partition_null_semantics() {
    let table = table();
    let expected_prefix = vec![
        JobSqlValue::String("job-1".into()),
        JobSqlValue::Integer(11),
        JobSqlValue::Integer(7),
        JobSqlValue::String("app".into()),
        JobSqlValue::String("events".into()),
    ];

    let (sql, args) = create_job_history_sql(
        "job-1",
        &table,
        None,
        "2026-09-13 11:00:00",
        "2026-09-13 12:00:00",
    );
    assert_eq!(sql, CREATE_JOB_HISTORY_SQL);
    assert_eq!(args[..5], expected_prefix);
    assert_eq!(args[5], JobSqlValue::Null);
    assert_eq!(args[6], JobSqlValue::String("2026-09-13 12:00:00".into()));
    assert_eq!(args[7], JobSqlValue::String("2026-09-13 11:00:00".into()));
    assert_eq!(args[8], JobSqlValue::String("running".into()));

    let (_, partitioned) = create_job_history_sql(
        "job-1",
        &table,
        Some("p0"),
        "2026-09-13 11:00:00",
        "2026-09-13 12:00:00",
    );
    assert_eq!(partitioned[5], JobSqlValue::String("p0".into()));
}

#[test]
fn local_finish_updates_only_the_current_job_and_cleans_tasks() {
    let mut current = job("job-1");
    let mut stale = job("stale");
    let mut store = JobStore::default();
    current.create_history(&mut store);
    store.active_jobs.insert(11, current.clone());
    store.tasks_by_job.insert(current.id.clone(), 3);

    let summary = TtlSummary {
        total_rows: 13,
        success_rows: 8,
        error_rows: 5,
        scan_task_err: "scan failed".into(),
    };
    assert!(!stale.finish(&mut store, 200, summary.clone()));
    assert!(!stale.finished);
    assert!(store.active_jobs.contains_key(&11));
    assert_eq!(store.tasks_by_job.get("job-1"), Some(&3));

    assert!(current.finish(&mut store, 200, summary.clone()));
    assert!(current.finished);
    assert!(!store.active_jobs.contains_key(&11));
    assert!(!store.tasks_by_job.contains_key("job-1"));
    let history = &store.history["job-1"];
    assert_eq!(history.finish_time, Some(200));
    assert_eq!(history.summary, Some(summary));
}

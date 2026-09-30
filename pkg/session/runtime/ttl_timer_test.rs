// Copyright 2026 AsterSQL.

use astersql_ttl_ttlworker::session::{Datum, PhysicalTable, WorkerSession};

use super::{
    CreateAnalyzeSession, ttl_metadata::TtlSchedule, ttl_timer::sync_ttl_timers,
    ttl_worker_session::TtlWorkerSqlSession,
};

#[test]
fn go_merge_43_ttl_timer_sync_persists_updates_and_disables() {
    let (_, session) = CreateAnalyzeSession().unwrap();
    let mut worker = TtlWorkerSqlSession::new(session);
    let mut schedule = TtlSchedule {
        table: PhysicalTable {
            partition_name: None,
            table_id: 4001,
            physical_id: 4001,
            schema: "test".into(),
            table: "timer_test".into(),
            key_columns: vec!["id".into()],
            ttl_column: "expire_at".into(),
            ttl_enabled: true,
            definition_version: 1,
            expire_after_seconds: 1,
        },
        job_interval_seconds: 86_400,
        job_interval_expression: "24h".into(),
    };
    sync_ttl_timers(&mut worker, std::slice::from_ref(&schedule), 200_000).unwrap();
    let read = |worker: &mut TtlWorkerSqlSession| {
        worker
            .execute(
                "SELECT SCHED_POLICY_EXPR,ENABLE FROM mysql.tidb_timers WHERE NAMESPACE='default' AND HOOK_CLASS='tidb.ttl'",
                &[],
            )
            .unwrap()
    };
    assert_eq!(
        read(&mut worker),
        vec![vec![Datum::Text("24h".into()), Datum::Text("1".into())]]
    );
    worker
        .execute(
            "UPDATE mysql.tidb_timers SET TIMER_EXT=%? WHERE NAMESPACE='default' AND TIMER_KEY=%?",
            &[
                Datum::Text("{\"tags\":[],\"manual\":{\"request_id\":\"request-1\"},\"event\":{\"manual_request_id\":\"request-1\"}}".into()),
                Datum::Text(astersql_ttl_ttlworker::timer_sync::timer_key(4001, 4001)),
            ],
        )
        .unwrap();
    schedule.job_interval_expression = "12h".into();
    sync_ttl_timers(&mut worker, std::slice::from_ref(&schedule), 200_001).unwrap();
    let ext = worker
        .execute(
            "SELECT TIMER_EXT FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
            &[],
        )
        .unwrap();
    let Datum::Text(ext) = &ext[0][0] else {
        panic!("timer ext must be JSON")
    };
    let ext: serde_json::Value = serde_json::from_str(ext).unwrap();
    assert_eq!(ext["manual"]["request_id"], "request-1");
    assert_eq!(ext["event"]["manual_request_id"], "request-1");
    assert_eq!(
        read(&mut worker),
        vec![vec![Datum::Text("12h".into()), Datum::Text("1".into())]]
    );
    sync_ttl_timers(&mut worker, &[], 200_002).unwrap();
    assert_eq!(
        read(&mut worker),
        vec![vec![Datum::Text("12h".into()), Datum::Text("0".into())]]
    );
    sync_ttl_timers(&mut worker, &[schedule], 200_003).unwrap();
    assert_eq!(
        read(&mut worker),
        vec![vec![Datum::Text("12h".into()), Datum::Text("1".into())]]
    );
}

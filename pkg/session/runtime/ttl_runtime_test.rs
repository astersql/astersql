// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::time::Duration;

use astersql_meta_model::TTLInfo;
use astersql_parser_ast::{NewCIStr, TimeUnitType};
use astersql_ttl_ttlworker::persistent::PersistentJobStore;
use astersql_ttl_ttlworker::session::{Datum, PhysicalTable, WorkerSession};

use super::{
    ConcreteSession, CreateAnalyzeSession,
    ttl_metadata::collect_physical_ttl_tables,
    ttl_runtime::{
        JobHeartbeat, persisted_scan_ranges, run_ttl_tick, start_domain_ttl_job_manager,
        within_ttl_window,
    },
    ttl_worker_session::TtlWorkerSqlSession,
};

#[test]
fn go_merge_43_ttl_schedule_window_honors_utc_and_midnight() {
    let hour = |hour: u64| 1_700_000_000_u64 - 1_700_000_000_u64 % 86_400 + hour * 3_600;
    assert!(within_ttl_window(hour(23), "22:00 +0000", "02:00 +0000").unwrap());
    assert!(within_ttl_window(hour(1), "22:00 +0000", "02:00 +0000").unwrap());
    assert!(!within_ttl_window(hour(12), "22:00 +0000", "02:00 +0000").unwrap());
}

#[test]
fn go_merge_43_ttl_periodic_heartbeat_detects_lost_ownership() {
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    let mut worker = TtlWorkerSqlSession::new(session);
    let table = PhysicalTable {
        partition_name: None,
        table_id: 999,
        physical_id: 999,
        schema: "test".into(),
        table: "heartbeat_test".into(),
        key_columns: vec!["id".into()],
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 1,
    };
    assert!(
        PersistentJobStore::start_job(
            &mut worker,
            &table,
            "owner-a",
            "heartbeat-job",
            200_000,
            None
        )
        .unwrap()
    );
    let mut heartbeat = JobHeartbeat::start(
        Arc::clone(&domain),
        table.physical_id,
        "heartbeat-job".into(),
        "owner-a".into(),
        Duration::from_millis(5),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let rows = worker
            .execute(
                "SELECT table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_owner_hb_time > FROM_UNIXTIME(%?)",
                &[Datum::Integer(999), Datum::Unsigned(200_000)],
            )
            .expect("read heartbeat");
        if !rows.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "heartbeat never advanced"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    worker
        .execute(
            "UPDATE mysql.tidb_ttl_table_status SET current_job_owner_id=%? WHERE table_id=%?",
            &[Datum::Text("owner-b".into()), Datum::Integer(999)],
        )
        .expect("simulate ownership takeover");
    while !heartbeat.lost() {
        assert!(
            std::time::Instant::now() < deadline,
            "ownership loss was not observed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    heartbeat.stop();
}

#[test]
fn go_merge_43_ttl_persists_region_scan_ranges() {
    let (_, session) = CreateAnalyzeSession().unwrap();
    let mut worker = TtlWorkerSqlSession::new(session);
    let table = PhysicalTable {
        partition_name: None,
        table_id: 1001,
        physical_id: 1001,
        schema: "test".into(),
        table: "range_test".into(),
        key_columns: vec!["id".into()],
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 1,
    };
    let ranges = [
        astersql_ttl_cache::table::ScanRange {
            Start: vec![],
            End: vec![astersql_ttl_cache::task::Datum::Int(5)],
        },
        astersql_ttl_cache::table::ScanRange {
            Start: vec![astersql_ttl_cache::task::Datum::Int(5)],
            End: vec![],
        },
    ];
    assert!(
        PersistentJobStore::start_job_with_ranges(
            &mut worker,
            &table,
            "owner-a",
            "range-job",
            200_000,
            None,
            &ranges,
        )
        .unwrap()
    );
    let rows = worker.execute(
        "SELECT scan_id,HEX(scan_range_start),HEX(scan_range_end) FROM mysql.tidb_ttl_task WHERE job_id=%? ORDER BY scan_id",
        &[Datum::Text("range-job".into())],
    ).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Datum::Text("0".into()));
    assert_eq!(rows[1][0], Datum::Text("1".into()));
    let loaded = persisted_scan_ranges(&mut worker, "range-job").unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].end, Some(vec![Datum::Integer(5)]));
    assert_eq!(loaded[1].start, Some(vec![Datum::Integer(5)]));
}

#[test]
fn go_merge_43_domain_owns_and_stops_real_ttl_job_manager() {
    let (domain, _) = CreateAnalyzeSession().expect("SQL session");
    assert!(start_domain_ttl_job_manager(&domain).expect("start TTL manager"));
    assert!(!start_domain_ttl_job_manager(&domain).expect("duplicate start is ignored"));
    domain.close();
    assert!(domain.is_closed());
}

#[test]
fn go_merge_43_ttl_tick_persists_job_and_deletes_expired_rows() {
    let (domain, setup) = CreateAnalyzeSession().expect("SQL session");
    setup
        .execute("CREATE TABLE ttl_runtime_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create table metadata template");
    let (_, mut table) = domain
        .stats_table("test", "ttl_runtime_template")
        .expect("canonical table metadata");
    table.ID = 0;
    table.Name = NewCIStr("ttl_runtime_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain
        .ddl_create_table("test", table, false)
        .expect("publish TTL table metadata");
    let session = ConcreteSession::new(Arc::clone(&domain));
    session
        .execute("INSERT INTO ttl_runtime_target VALUES (1, '1970-01-01 00:00:01'), (2, '1970-01-04 00:00:00')")
        .expect("insert expired and live rows");

    let result = run_ttl_tick(&domain, "owner-a", 200_000, || false).expect("run TTL manager tick");
    assert_eq!(result.tables, 1);
    assert_eq!(result.claimed, 1);
    assert_eq!(result.finished, 1);
    let mut timers = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    let timer_rows = timers
        .execute(
            "SELECT TIMER_KEY FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
            &[],
        )
        .expect("read durable TTL timers");
    assert_eq!(timer_rows.len(), 1, "TTL schedule must persist a timer");
    let event_rows = timers
        .execute(
            "SELECT EVENT_STATUS,SUMMARY_DATA FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
            &[],
        )
        .expect("read durable TTL event result");
    assert_eq!(event_rows[0][0], Datum::Text("IDLE".into()));
    assert_ne!(event_rows[0][1], Datum::Text("<nil>".into()));
    let rows = session
        .execute("SELECT id FROM ttl_runtime_target ORDER BY id")
        .expect("read TTL table");
    let remaining = rows
        .into_iter()
        .flat_map(|set| set.rows)
        .collect::<Vec<_>>();
    assert_eq!(remaining, vec![vec!["2".to_owned()]]);
    let second = run_ttl_tick(&domain, "owner-a", 200_001, || false)
        .expect("job interval prevents rescheduling");
    assert_eq!(second.claimed, 0);
}

#[test]
fn go_merge_43_ttl_canceled_scan_preserves_durable_job() {
    let (domain, setup) = CreateAnalyzeSession().expect("SQL session");
    setup
        .execute("CREATE TABLE ttl_cancel_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create template");
    let (_, mut table) = domain.stats_table("test", "ttl_cancel_template").unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_cancel_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain.ddl_create_table("test", table, false).unwrap();
    let checks = std::cell::Cell::new(0);
    let result = run_ttl_tick(&domain, "owner-a", 200_000, || {
        checks.set(checks.get() + 1);
        checks.get() >= 3
    });
    assert!(result.is_err(), "canceled scan must remain resumable");
    let rows = setup
        .execute("SELECT current_job_id FROM mysql.tidb_ttl_table_status WHERE current_job_id IS NOT NULL")
        .unwrap();
    assert_eq!(rows.into_iter().flat_map(|set| set.rows).count(), 1);
    let timer = setup
        .execute("SELECT EVENT_STATUS FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'")
        .unwrap();
    assert_eq!(
        timer
            .into_iter()
            .flat_map(|set| set.rows)
            .collect::<Vec<_>>(),
        vec![vec!["TRIGGER".to_owned()]]
    );
}

#[test]
fn go_merge_43_ttl_tick_resumes_expired_owner_job_with_original_watermark() {
    let (domain, setup) = CreateAnalyzeSession().expect("SQL session");
    setup
        .execute("CREATE TABLE ttl_resume_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create template");
    let (_, mut table) = domain
        .stats_table("test", "ttl_resume_template")
        .expect("table metadata");
    table.ID = 0;
    table.Name = NewCIStr("ttl_resume_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain
        .ddl_create_table("test", table, false)
        .expect("publish TTL metadata");
    let physical = collect_physical_ttl_tables(domain.info_schema().as_ref(), 200_000)
        .expect("TTL physical table")
        .into_iter()
        .find(|item| item.table == "ttl_resume_target")
        .expect("target");
    let mut owner = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    assert!(
        PersistentJobStore::start_job(
            &mut owner,
            &physical,
            "dead-owner",
            "resume-job",
            200_000,
            None
        )
        .unwrap()
    );
    setup.execute("INSERT INTO ttl_resume_target VALUES (1, '1970-01-01 00:00:01'), (2, '1970-01-03 00:00:00')").expect("insert rows");
    let result =
        run_ttl_tick(&domain, "new-owner", 200_300, || false).expect("resume stale TTL job");
    assert_eq!(result.claimed, 0);
    assert_eq!(result.resumed, 1);
    assert_eq!(result.finished, 1);
    let rows = owner
        .execute("SELECT id FROM ttl_resume_target ORDER BY id", &[])
        .unwrap();
    assert_eq!(
        rows,
        vec![vec![astersql_ttl_ttlworker::session::Datum::Text(
            "2".into()
        )]]
    );
}

#[test]
fn go_merge_43_ttl_takeover_resumes_after_persisted_cursor() {
    let (domain, setup) = CreateAnalyzeSession().unwrap();
    setup
        .execute("CREATE TABLE ttl_cursor_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .unwrap();
    let (_, mut table) = domain.stats_table("test", "ttl_cursor_template").unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_cursor_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain.ddl_create_table("test", table, false).unwrap();
    let physical = collect_physical_ttl_tables(domain.info_schema().as_ref(), 200_000)
        .unwrap()
        .into_iter()
        .find(|item| item.table == "ttl_cursor_target")
        .unwrap();
    let mut owner = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    assert!(
        PersistentJobStore::start_job(
            &mut owner,
            &physical,
            "dead-owner",
            "cursor-job",
            200_000,
            None
        )
        .unwrap()
    );
    owner
        .execute(
            "UPDATE mysql.tidb_ttl_task SET state=%? WHERE job_id=%? AND scan_id=0",
            &[
                Datum::Text(
                    r#"{"cursor":["1"],"total_rows":1,"success_rows":1,"error_rows":0}"#.into(),
                ),
                Datum::Text("cursor-job".into()),
            ],
        )
        .unwrap();
    setup.execute("INSERT INTO ttl_cursor_target VALUES (1, '1970-01-01 00:00:01'), (2, '1970-01-01 00:00:01')").unwrap();
    let result = run_ttl_tick(&domain, "new-owner", 200_300, || false).unwrap();
    assert_eq!(result.resumed, 1);
    let rows = owner
        .execute("SELECT id FROM ttl_cursor_target ORDER BY id", &[])
        .unwrap();
    assert_eq!(rows, vec![vec![Datum::Text("1".into())]]);
    let summary = owner
        .execute(
            "SELECT last_job_summary FROM mysql.tidb_ttl_table_status WHERE table_id=%?",
            &[Datum::Integer(physical.physical_id)],
        )
        .unwrap();
    let Datum::Text(summary) = &summary[0][0] else {
        panic!("missing TTL summary")
    };
    assert!(summary.contains("\"total_rows\":2"), "{summary}");
    assert!(summary.contains("\"success_rows\":2"), "{summary}");
}

#[test]
fn go_merge_43_ttl_takeover_scans_all_persisted_ranges() {
    let (domain, setup) = CreateAnalyzeSession().unwrap();
    setup
        .execute("CREATE TABLE ttl_ranges_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .unwrap();
    let (_, mut table) = domain.stats_table("test", "ttl_ranges_template").unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_ranges_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain.ddl_create_table("test", table, false).unwrap();
    let physical = collect_physical_ttl_tables(domain.info_schema().as_ref(), 200_000)
        .unwrap()
        .into_iter()
        .find(|item| item.table == "ttl_ranges_target")
        .unwrap();
    let mut owner = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    let ranges = [
        astersql_ttl_cache::table::ScanRange {
            Start: vec![],
            End: vec![astersql_ttl_cache::task::Datum::Int(2)],
        },
        astersql_ttl_cache::table::ScanRange {
            Start: vec![astersql_ttl_cache::task::Datum::Int(2)],
            End: vec![],
        },
    ];
    assert!(
        PersistentJobStore::start_job_with_ranges(
            &mut owner,
            &physical,
            "dead-owner",
            "ranges-job",
            200_000,
            None,
            &ranges,
        )
        .unwrap()
    );
    setup.execute("INSERT INTO ttl_ranges_target VALUES (1, '1970-01-01 00:00:01'), (2, '1970-01-01 00:00:01')").unwrap();
    let result = run_ttl_tick(&domain, "new-owner", 200_300, || false).unwrap();
    assert_eq!(result.resumed, 1);
    assert_eq!(result.finished, 1);
    assert!(
        owner
            .execute("SELECT id FROM ttl_ranges_target", &[])
            .unwrap()
            .is_empty()
    );
}

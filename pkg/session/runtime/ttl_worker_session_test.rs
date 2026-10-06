// Copyright 2026 AsterSQL.

use astersql_ttl_ttlworker::del::{DeleteRateLimiter, DeleteTask};
use astersql_ttl_ttlworker::job_manager::TtlSummary;
use astersql_ttl_ttlworker::persistent::PersistentJobStore;
use astersql_ttl_ttlworker::scan::{TaskTerminateReason, TtlScanTask, TtlStatistics};
use astersql_ttl_ttlworker::session::{Datum, WorkerSession};
use astersql_ttl_ttlworker::session::{PhysicalTable, SessionError};
use std::sync::Arc;

use super::ttl_worker_session::TtlWorkerSqlSession;
use super::{ConcreteSession, CreateAnalyzeSession};

#[test]
fn go_merge_43_ttl_worker_uses_real_sql_session_and_binds_values() {
    let (_, session) = CreateAnalyzeSession().expect("SQL session");
    session
        .execute("CREATE TABLE ttl_adapter_test (id INT PRIMARY KEY, value VARCHAR(40))")
        .expect("create user table");
    let mut worker = TtlWorkerSqlSession::new(session);
    worker
        .execute(
            "INSERT INTO ttl_adapter_test VALUES (%?, %?)",
            &[Datum::Integer(7), Datum::Text("a'b\\c".into())],
        )
        .expect("insert with bound arguments");
    let rows = worker
        .execute(
            "SELECT value FROM ttl_adapter_test WHERE id = %?",
            &[Datum::Integer(7)],
        )
        .expect("read through worker session");
    assert_eq!(rows, vec![vec![Datum::Text("a'b\\c".into())]]);
    assert!(worker.execute("SELECT %?", &[]).is_err());
}

#[test]
fn go_merge_43_ttl_worker_scans_and_deletes_only_one_partition() {
    let (_, session) = CreateAnalyzeSession().expect("SQL session");
    session
        .execute("CREATE TABLE ttl_partition_test (id INT PRIMARY KEY, expire_at DATETIME) PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (10), PARTITION p1 VALUES LESS THAN MAXVALUE)")
        .expect("create partitioned table");
    session
        .execute("INSERT INTO ttl_partition_test VALUES (1, '2020-01-01'), (11, '2020-01-01')")
        .expect("insert expired rows in both partitions");
    let mut worker = TtlWorkerSqlSession::new(session);
    let table = PhysicalTable {
        partition_name: Some("p0".into()),
        table_id: 1,
        physical_id: 2,
        schema: "test".into(),
        table: "ttl_partition_test".into(),
        key_columns: vec!["id".into()],
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 1,
    };
    let scan = TtlScanTask {
        job_id: "partition-job".into(),
        scan_id: 0,
        table: table.clone(),
        expire_time: 1_700_000_000,
        range_start: None,
        range_end: None,
        batch_size: 100,
        scan_index: None,
    };
    let statistics = Arc::new(TtlStatistics::default());
    let mut scanned = Vec::new();
    let result = scan.execute(
        &mut worker,
        &statistics,
        |rows| {
            scanned.extend(rows);
            Ok(())
        },
        || false,
    );
    assert_eq!(result.reason, TaskTerminateReason::Finished, "{result:?}");
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0], vec![Datum::Text("1".into())]);
    struct NoLimit;
    impl DeleteRateLimiter for NoLimit {
        fn wait_delete_token(&mut self, _: usize) -> Result<(), SessionError> {
            Ok(())
        }
    }
    let delete = DeleteTask {
        job_id: "partition-job".into(),
        table,
        rows: scanned,
        expire_time: scan.expire_time,
        statistics,
    };
    assert!(delete.do_delete(&mut worker, &mut NoLimit).is_empty());
    let rows = worker
        .execute("SELECT id FROM ttl_partition_test ORDER BY id", &[])
        .expect("read remaining partition");
    assert_eq!(rows, vec![vec![Datum::Text("11".into())]]);
}

#[test]
fn go_merge_43_ttl_system_tables_are_queryable_from_worker_session() {
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    let mut worker = TtlWorkerSqlSession::new(session);
    for table in [
        "tidb_ttl_table_status",
        "tidb_ttl_task",
        "tidb_ttl_job_history",
    ] {
        worker
            .execute(&format!("SELECT * FROM mysql.{table} LIMIT 1"), &[])
            .unwrap_or_else(|error| panic!("query mysql.{table}: {error:?}"));
    }
    worker
        .execute(
            astersql_ttl_ttlworker::job_manager::INSERT_NEW_TABLE_INTO_STATUS_SQL,
            &[Datum::Integer(901), Datum::Integer(900)],
        )
        .expect("persist table status");
    worker
        .execute("BEGIN PESSIMISTIC", &[])
        .expect("begin TTL job transaction");
    worker
        .execute(
            "SELECT table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? FOR UPDATE NOWAIT",
            &[Datum::Integer(901)],
        )
        .expect("lock status row without waiting");
    worker
        .execute("COMMIT", &[])
        .expect("commit TTL job transaction");
    let mut another = TtlWorkerSqlSession::new(ConcreteSession::new(domain));
    assert_eq!(
        another
            .execute(
                "SELECT table_id,parent_table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%?",
                &[Datum::Integer(901)],
            )
            .expect("read persisted status from another session"),
        vec![vec![Datum::Text("901".into()), Datum::Text("900".into())]],
    );
}

#[test]
fn go_merge_43_ttl_job_claim_persists_status_history_and_scan_task() {
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    let table = PhysicalTable {
        partition_name: None,
        table_id: 902,
        physical_id: 902,
        schema: "test".into(),
        table: "ttl_persistent_test".into(),
        key_columns: vec!["id".into()],
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 30,
    };
    let mut worker = TtlWorkerSqlSession::new(session);
    assert!(
        PersistentJobStore::start_job(&mut worker, &table, "owner-a", "job-a", 100, None)
            .expect("claim TTL job")
    );
    let mut another = TtlWorkerSqlSession::new(ConcreteSession::new(domain));
    assert_eq!(
        another
            .execute(
                "SELECT current_job_id,current_job_owner_id FROM mysql.tidb_ttl_table_status WHERE table_id=%?",
                &[Datum::Integer(902)],
            )
            .expect("read claim"),
        vec![vec![Datum::Text("job-a".into()), Datum::Text("owner-a".into())]],
    );
    assert_eq!(
        another
            .execute(
                "SELECT job_id FROM mysql.tidb_ttl_job_history WHERE job_id=%?",
                &[Datum::Text("job-a".into())]
            )
            .expect("read history"),
        vec![vec![Datum::Text("job-a".into())]],
    );
    assert_eq!(
        another
            .execute(
                "SELECT job_id FROM mysql.tidb_ttl_task WHERE job_id=%?",
                &[Datum::Text("job-a".into())]
            )
            .expect("read task"),
        vec![vec![Datum::Text("job-a".into())]],
    );
    assert!(
        !PersistentJobStore::start_job(&mut another, &table, "owner-b", "job-b", 101, None)
            .expect("competing claim is rejected")
    );
    assert!(
        !PersistentJobStore::heartbeat(&mut another, 902, "job-a", "owner-b", 110)
            .expect("foreign owner cannot heartbeat")
    );
    assert!(
        PersistentJobStore::heartbeat(&mut another, 902, "job-a", "owner-a", 110)
            .expect("owner heartbeat")
    );
    assert!(
        PersistentJobStore::heartbeat(&mut another, 902, "job-a", "owner-a", 110)
            .expect("repeated heartbeat in same second")
    );
    assert_eq!(
        PersistentJobStore::takeover_timeout(&mut another, 902, "owner-b", 120, 30)
            .expect("live job is retained"),
        None,
    );
    assert_eq!(
        PersistentJobStore::takeover_timeout(&mut another, 902, "owner-b", 200, 30)
            .expect("stale job is taken over"),
        Some("job-a".into()),
    );
    assert_eq!(
        another
            .execute(
                "SELECT current_job_owner_id FROM mysql.tidb_ttl_table_status WHERE table_id=%?",
                &[Datum::Integer(902)],
            )
            .expect("read new owner"),
        vec![vec![Datum::Text("owner-b".into())]],
    );
    assert!(
        PersistentJobStore::finish_job(
            &mut worker,
            table.physical_id,
            "job-a",
            "owner-a",
            205,
            &TtlSummary::default(),
            "{}",
        )
        .is_err(),
        "the previous owner must not finish a taken-over job"
    );
    PersistentJobStore::finish_job(
        &mut another,
        table.physical_id,
        "job-a",
        "owner-b",
        210,
        &TtlSummary {
            total_rows: 1,
            success_rows: 1,
            error_rows: 0,
            scan_task_err: String::new(),
        },
        r#"{"total_rows":1,"success_rows":1,"error_rows":0}"#,
    )
    .expect("finish persisted TTL job");
    assert_eq!(
        another
            .execute(
                "SELECT last_job_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_id IS NULL",
                &[Datum::Integer(902)],
            )
            .expect("read completed status"),
        vec![vec![Datum::Text("job-a".into())]],
    );
    assert!(
        another
            .execute(
                "SELECT job_id FROM mysql.tidb_ttl_task WHERE job_id=%?",
                &[Datum::Text("job-a".into())]
            )
            .expect("scan task removed")
            .is_empty()
    );
    assert_eq!(
        another
            .execute(
                "SELECT status,expired_rows,deleted_rows FROM mysql.tidb_ttl_job_history WHERE job_id=%?",
                &[Datum::Text("job-a".into())],
            )
            .expect("read finished history"),
        vec![vec![
            Datum::Text("finished".into()),
            Datum::Text("1".into()),
            Datum::Text("1".into()),
        ]],
    );
    assert!(
        !PersistentJobStore::start_job(&mut another, &table, "owner-b", "job-b", 220, Some(200),)
            .expect("job interval blocks early restart")
    );
    assert!(
        PersistentJobStore::start_job(&mut another, &table, "owner-b", "job-b", 301, Some(200),)
            .expect("job interval elapsed")
    );
}

struct UnlimitedDeleteRate;

impl DeleteRateLimiter for UnlimitedDeleteRate {
    fn wait_delete_token(&mut self, _rows: usize) -> Result<(), SessionError> {
        Ok(())
    }
}

#[test]
fn go_merge_43_ttl_scan_and_delete_expired_rows_through_real_sql() {
    let (domain, session) = CreateAnalyzeSession().expect("SQL session");
    session
        .execute("CREATE TABLE ttl_adapter_expiry (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create user table");
    session
        .execute(
            "INSERT INTO ttl_adapter_expiry VALUES (1, '1970-01-01 00:00:01'), (2, '1970-01-01 00:16:40')",
        )
        .expect("insert expired and live rows");

    let table = PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "ttl_adapter_expiry".into(),
        key_columns: vec!["id".into()],
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 0,
    };
    let task = TtlScanTask {
        job_id: "job-1".into(),
        scan_id: 0,
        table: table.clone(),
        expire_time: 100,
        range_start: None,
        range_end: None,
        batch_size: 16,
        scan_index: None,
    };
    let statistics = Arc::new(TtlStatistics::default());
    let mut scan_worker = TtlWorkerSqlSession::new(session);
    let mut delete_worker = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    assert_eq!(
        delete_worker
            .execute(
                "SELECT id FROM ttl_adapter_expiry WHERE id IN ('1') AND expire_at < '1970-01-01 00:01:40'",
                &[],
            )
            .expect("check delete predicate"),
        vec![vec![Datum::Text("1".into())]],
    );
    let mut limiter = UnlimitedDeleteRate;
    let result = task.execute(
        &mut scan_worker,
        &statistics,
        |rows| {
            let delete = DeleteTask {
                job_id: task.job_id.clone(),
                table: table.clone(),
                rows,
                expire_time: task.expire_time,
                statistics: Arc::clone(&statistics),
            };
            if delete
                .do_delete(&mut delete_worker, &mut limiter)
                .is_empty()
            {
                Ok(())
            } else {
                Err(SessionError::Execute("TTL delete needs retry".into()))
            }
        },
        || false,
    );
    assert_eq!(
        result.reason,
        TaskTerminateReason::Finished,
        "{:?}",
        result.error
    );
    assert_eq!(statistics.snapshot(), (1, 1, 0));
    let rows = scan_worker
        .execute("SELECT id FROM ttl_adapter_expiry ORDER BY id", &[])
        .expect("read remaining rows");
    assert_eq!(rows, vec![vec![Datum::Text("2".into())]]);
}

#[test]
fn ttl_datetime_cutoff_uses_captured_global_wall_clock_in_utc() {
    let (_, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("SET @@global.time_zone='America/New_York'")
        .unwrap();
    session.execute("SET @@time_zone='UTC'").unwrap();
    session.execute("CREATE TABLE ttl_wall_clock (id BIGINT PRIMARY KEY, expire_at DATETIME(6)) TTL=expire_at + INTERVAL 1 HOUR").unwrap();
    session.execute("INSERT INTO ttl_wall_clock VALUES (1, '2024-11-03 01:29:59'), (2, '2024-11-03 01:30:00'), (3, '2024-11-03 01:30:01')").unwrap();
    let mut worker = TtlWorkerSqlSession::new(session);
    let table = PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: "ttl_wall_clock".into(),
        key_columns: vec!["id".into()],
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 3600,
    };
    let task = TtlScanTask {
        job_id: "wall-clock".into(),
        scan_id: 0,
        table,
        expire_time: 1_730_615_400,
        range_start: None,
        range_end: None,
        batch_size: 1,
        scan_index: None,
    };
    let mut scanned = Vec::new();
    let result = task.execute(
        &mut worker,
        &TtlStatistics::default(),
        |rows| {
            scanned.extend(rows);
            Ok(())
        },
        || false,
    );
    assert_eq!(result.reason, TaskTerminateReason::Finished, "{result:?}");
    assert_eq!(scanned, vec![vec![Datum::Text("1".into())]]);
}

fn temporal_table(name: &str, keys: Vec<String>) -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".into(),
        table: name.into(),
        key_columns: keys,
        ttl_column: "expire_at".into(),
        ttl_enabled: true,
        definition_version: 1,
        expire_after_seconds: 3600,
    }
}

#[test]
fn ttl_timestamp_pagination_preserves_instants_across_time_zones() {
    for (zone, instants) in [
        (
            "America/New_York",
            [
                "2024-11-03 05:30:00.123456",
                "2024-11-03 06:30:00.123456",
                "2024-11-03 07:30:00.123456",
            ],
        ),
        (
            "America/New_York",
            [
                "2024-03-10 06:30:00.123456",
                "2024-03-10 07:30:00.123456",
                "2024-03-10 08:30:00.123456",
            ],
        ),
        (
            "+05:30",
            [
                "2024-01-01 00:00:00.123456",
                "2024-01-01 01:00:00.123456",
                "2024-01-01 02:00:00.123456",
            ],
        ),
        (
            "Asia/Shanghai",
            [
                "2024-01-01 00:00:00.123456",
                "2024-01-01 01:00:00.123456",
                "2024-01-01 02:00:00.123456",
            ],
        ),
    ] {
        let (_, session) = CreateAnalyzeSession().unwrap();
        session
            .execute(&format!("SET @@global.time_zone='{zone}'"))
            .unwrap();
        session.execute("SET @@time_zone='UTC'").unwrap();
        session.execute("CREATE TABLE ttl_instants (expire_at TIMESTAMP(6) NOT NULL, id BIGINT NOT NULL, PRIMARY KEY(expire_at, id) CLUSTERED) TTL=expire_at + INTERVAL 1 HOUR").unwrap();
        for (i, instant) in instants.iter().enumerate() {
            session
                .execute(&format!(
                    "INSERT INTO ttl_instants VALUES ('{instant}', {})",
                    i + 1
                ))
                .unwrap();
        }
        let mut worker = TtlWorkerSqlSession::new(session);
        let task = TtlScanTask {
            job_id: "instants".into(),
            scan_id: 0,
            table: temporal_table("ttl_instants", vec!["expire_at".into(), "id".into()]),
            expire_time: 1735689600,
            range_start: None,
            range_end: None,
            batch_size: 1,
            scan_index: None,
        };
        let mut scanned = Vec::new();
        let result = task.execute(
            &mut worker,
            &TtlStatistics::default(),
            |rows| {
                scanned.extend(rows);
                Ok(())
            },
            || false,
        );
        assert_eq!(
            result.reason,
            TaskTerminateReason::Finished,
            "{zone}: {result:?}"
        );
        assert_eq!(scanned.len(), 3, "{zone}: {scanned:?}");
        for (i, row) in scanned.iter().enumerate() {
            assert_eq!(
                row,
                &vec![
                    Datum::Text(instants[i].into()),
                    Datum::Text((i + 1).to_string())
                ]
            );
        }
    }
}

#[test]
fn ttl_delete_rechecks_strict_temporal_boundary_after_global_timezone_changes() {
    struct NoLimit;
    impl DeleteRateLimiter for NoLimit {
        fn wait_delete_token(&mut self, _: usize) -> Result<(), SessionError> {
            Ok(())
        }
    }
    for (kind, frontier, values, updated) in [
        (
            "TIMESTAMP",
            1730615400,
            [
                "2024-11-03 05:30:00",
                "2024-11-03 06:30:00",
                "2024-11-03 07:30:00",
                "2024-11-03 05:45:00",
            ],
            "2024-11-03 07:45:00",
        ),
        (
            "DATETIME",
            1730615400,
            [
                "2024-11-03 01:29:59",
                "2024-11-03 01:30:00",
                "2024-11-03 01:30:01",
                "2024-11-03 01:00:00",
            ],
            "2024-11-03 02:00:00",
        ),
        (
            "DATE",
            1730606400,
            ["2024-11-02", "2024-11-03", "2024-11-04", "2024-11-01"],
            "2024-11-05",
        ),
        (
            "DATE",
            1730653200,
            ["2024-11-02", "2024-11-03", "2024-11-04", "2024-11-01"],
            "2024-11-05",
        ),
    ] {
        let (domain, session) = CreateAnalyzeSession().unwrap();
        session
            .execute("SET @@global.time_zone='America/New_York'")
            .unwrap();
        session.execute("SET @@time_zone='UTC'").unwrap();
        session.execute(&format!("CREATE TABLE ttl_delete_boundary (id BIGINT PRIMARY KEY CLUSTERED, expire_at {kind} NOT NULL) TTL=expire_at + INTERVAL 1 HOUR")).unwrap();
        for (i, value) in values.iter().enumerate() {
            session
                .execute(&format!(
                    "INSERT INTO ttl_delete_boundary VALUES ({}, '{value}')",
                    i + 1
                ))
                .unwrap();
        }
        let table = temporal_table("ttl_delete_boundary", vec!["id".into()]);
        let mut scan_session = TtlWorkerSqlSession::new(session);
        let captured = scan_session.expiration_predicate(&table, frontier).unwrap();
        let mut delete_session = TtlWorkerSqlSession::new(ConcreteSession::new(domain));
        delete_session
            .execute("SET @@time_zone='UTC'", &[])
            .unwrap();
        delete_session.use_expiration(&table, frontier, captured);
        scan_session
            .execute("SET @@global.time_zone='+08:00'", &[])
            .unwrap();
        scan_session
            .execute(
                "UPDATE ttl_delete_boundary SET expire_at=%? WHERE id=4",
                &[Datum::Text(updated.into())],
            )
            .unwrap();
        let statistics = Arc::new(TtlStatistics::default());
        let task = DeleteTask {
            job_id: "boundary".into(),
            table,
            rows: (1..=4).map(|id| vec![Datum::Integer(id)]).collect(),
            expire_time: frontier,
            statistics,
        };
        assert!(
            task.do_delete(&mut delete_session, &mut NoLimit).is_empty(),
            "{kind}"
        );
        let rows = delete_session
            .execute("SELECT id FROM ttl_delete_boundary ORDER BY id", &[])
            .unwrap();
        let remaining = if kind == "DATE" && frontier == 1730653200 {
            vec!["3", "4"]
        } else {
            vec!["2", "3", "4"]
        };
        assert_eq!(
            rows,
            remaining
                .into_iter()
                .map(|id| vec![Datum::Text(id.into())])
                .collect::<Vec<_>>(),
            "{kind} frontier={frontier}"
        );
    }
}

#[test]
fn ttl_sql_session_preparation_restores_real_variables() {
    use astersql_ttl_ttlworker::session::{prepare_session_checked, restore_session_checked};
    let (_, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("SET SESSION time_zone='America/New_York'")
        .unwrap();
    let mut compatibility_session = TtlWorkerSqlSession::new(session);
    assert_eq!(
        compatibility_session
            .execute("SELECT @@time_zone", &[])
            .unwrap(),
        vec![vec![Datum::Text("America/New_York".into())]]
    );
    for zone in ["SYSTEM", "+08:00", "Asia/Shanghai"] {
        let (_, session) = CreateAnalyzeSession().unwrap();
        for sql in [
            format!("SET @@time_zone='{zone}'"),
            "SET tidb_retry_limit=7".into(),
            "SET tidb_enable_1pc=OFF".into(),
            "SET tidb_enable_async_commit=OFF".into(),
            "SET tidb_isolation_read_engines='tikv'".into(),
        ] {
            session.execute(&sql).unwrap();
        }
        let mut worker = TtlWorkerSqlSession::new(session);
        let previous = prepare_session_checked(&mut worker).unwrap();
        assert_eq!(
            worker.execute("SELECT @@tidb_retry_limit", &[]).unwrap(),
            vec![vec![Datum::Text("0".into())]]
        );
        assert_eq!(
            worker.execute("SELECT @@time_zone", &[]).unwrap(),
            vec![vec![Datum::Text("UTC".into())]]
        );
        let scan_previous =
            astersql_ttl_ttlworker::session::prepare_scan_session_checked(&mut worker).unwrap();
        assert_eq!(
            worker
                .execute("SELECT @@tidb_distsql_scan_concurrency", &[])
                .unwrap(),
            vec![vec![Datum::Text("1".into())]]
        );
        astersql_ttl_ttlworker::session::restore_scan_session_checked(&mut worker, scan_previous)
            .unwrap();
        assert_eq!(
            worker
                .execute("SELECT @@tidb_distsql_scan_concurrency", &[])
                .unwrap(),
            vec![vec![Datum::Text("15".into())]]
        );
        restore_session_checked(&mut worker, previous).unwrap();
        assert!(worker.reusable());
        assert_eq!(
            worker.execute("SELECT @@time_zone", &[]).unwrap(),
            vec![vec![Datum::Text(zone.into())]]
        );
        assert_eq!(
            worker.execute("SELECT @@tidb_retry_limit", &[]).unwrap(),
            vec![vec![Datum::Text("7".into())]]
        );
        assert_eq!(
            worker
                .execute("SELECT @@tidb_isolation_read_engines", &[])
                .unwrap(),
            vec![vec![Datum::Text("tikv".into())]]
        );
    }
}

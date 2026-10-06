// Copyright 2026 AsterSQL.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use astersql_domain::{Domain, DomainConfig, KvInfoSchemaLoader};
use astersql_meta_model::TTLInfo;
use astersql_parser_ast::{NewCIStr, TimeUnitType};
use astersql_ttl_ttlworker::persistent::PersistentJobStore;
use astersql_ttl_ttlworker::session::{Datum, PhysicalTable, WorkerSession};

use super::{
    ConcreteSession, CreateAnalyzeSession,
    ttl_metadata::collect_physical_ttl_tables,
    ttl_runtime::{
        EtcdTtlWatchTransport, JobHeartbeat, TtlWatchEvent, TtlWatchKind, TtlWatchRuntime,
        TtlWatchTransport, persisted_scan_ranges, run_ttl_event, run_ttl_tick,
        start_domain_ttl_job_manager, start_domain_ttl_job_manager_with_interval,
        within_ttl_window,
    },
    ttl_timer::sync_ttl_timers,
    ttl_worker_session::TtlWorkerSqlSession,
};

struct ReconnectingTtlWatchFake {
    commands: Mutex<VecDeque<mpsc::Receiver<Vec<u8>>>>,
    scans: Mutex<VecDeque<mpsc::Receiver<Vec<u8>>>>,
    command_subscriptions: AtomicUsize,
    scan_subscriptions: AtomicUsize,
    responses: Mutex<Vec<(String, Result<serde_json::Value, String>)>>,
}

impl TtlWatchTransport for ReconnectingTtlWatchFake {
    fn watch(
        &self,
        kind: TtlWatchKind,
        _stopped: Arc<AtomicBool>,
    ) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        let (queue, count) = match kind {
            TtlWatchKind::Command => (&self.commands, &self.command_subscriptions),
            TtlWatchKind::Scan => (&self.scans, &self.scan_subscriptions),
        };
        count.fetch_add(1, Ordering::SeqCst);
        queue
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "no watch subscription prepared".to_owned())
    }

    fn take_command(&self, _request_id: &str) -> Result<bool, String> {
        Ok(true)
    }

    fn response_command(
        &self,
        request_id: &str,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        self.responses
            .lock()
            .unwrap()
            .push((request_id.to_owned(), result));
        Ok(())
    }
}

#[test]
fn go_merge_43_ttl_watch_reconnects_and_wakes_job_manager() {
    let (first_command, first_command_rx) = mpsc::channel();
    let (second_command, second_command_rx) = mpsc::channel();
    let (first_scan, first_scan_rx) = mpsc::channel();
    let (second_scan, second_scan_rx) = mpsc::channel();
    let transport = Arc::new(ReconnectingTtlWatchFake {
        commands: Mutex::new(VecDeque::from([first_command_rx, second_command_rx])),
        scans: Mutex::new(VecDeque::from([first_scan_rx, second_scan_rx])),
        command_subscriptions: AtomicUsize::new(0),
        scan_subscriptions: AtomicUsize::new(0),
        responses: Mutex::new(Vec::new()),
    });
    let mut watcher = TtlWatchRuntime::start(transport.clone(), std::thread::current());
    first_command
        .send(br#"{"request_id":"req-1","cmd_type":"trigger_ttl_job","data":{"db_name":"test","table_name":"ttl_watch"}}"#.to_vec())
        .unwrap();
    first_scan.send(b"task-1".to_vec()).unwrap();
    let mut received = vec![watcher.recv_timeout(Duration::from_secs(2)).unwrap()];
    received.push(watcher.recv_timeout(Duration::from_secs(2)).unwrap());
    assert!(received.contains(&TtlWatchEvent::Command {
        request_id: "req-1".into(),
        db_name: "test".into(),
        table_name: "ttl_watch".into(),
    }));
    assert!(received.contains(&TtlWatchEvent::Scan));
    drop(first_command);
    drop(first_scan);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while (transport.command_subscriptions.load(Ordering::SeqCst) < 2
        || transport.scan_subscriptions.load(Ordering::SeqCst) < 2)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(transport.command_subscriptions.load(Ordering::SeqCst), 2);
    assert_eq!(transport.scan_subscriptions.load(Ordering::SeqCst), 2);
    second_command
        .send(br#"{"request_id":"req-2","cmd_type":"trigger_ttl_job","data":{"db_name":"test","table_name":"ttl_watch"}}"#.to_vec())
        .unwrap();
    second_scan.send(b"task-2".to_vec()).unwrap();
    let mut received = vec![watcher.recv_timeout(Duration::from_secs(2)).unwrap()];
    received.push(watcher.recv_timeout(Duration::from_secs(2)).unwrap());
    assert!(received.contains(&TtlWatchEvent::Command {
        request_id: "req-2".into(),
        db_name: "test".into(),
        table_name: "ttl_watch".into(),
    }));
    assert!(received.contains(&TtlWatchEvent::Scan));
    watcher.stop();
    assert!(watcher.recv_timeout(Duration::from_millis(30)).is_err());

    let (command_sender, command_receiver) = mpsc::channel();
    let (scan_sender, scan_receiver) = mpsc::channel();
    let transport = Arc::new(ReconnectingTtlWatchFake {
        commands: Mutex::new(VecDeque::from([command_receiver])),
        scans: Mutex::new(VecDeque::from([scan_receiver])),
        command_subscriptions: AtomicUsize::new(0),
        scan_subscriptions: AtomicUsize::new(0),
        responses: Mutex::new(Vec::new()),
    });
    let (domain, setup) = CreateAnalyzeSession().expect("SQL domain");
    setup
        .execute("CREATE TABLE ttl_watch_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .unwrap();
    let (_, mut table) = domain.stats_table("test", "ttl_watch_template").unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_watch_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain.ddl_create_table("test", table, false).unwrap();
    assert!(
        start_domain_ttl_job_manager_with_interval(
            &domain,
            Some(transport.clone()),
            Duration::from_secs(60),
        )
        .unwrap()
    );
    let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let history = sql
            .execute(
                "SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_watch_target'",
                &[],
            )
            .unwrap();
        let timer = sql
            .execute(
                "SELECT EVENT_STATUS FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
                &[],
            )
            .unwrap();
        if history.len() == 1
            && history[0][1] == Datum::Text("finished".into())
            && timer.first().and_then(|row| row.first()) == Some(&Datum::Text("IDLE".into()))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "baseline timer did not finish"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let (_, mut second_table) = domain.stats_table("test", "ttl_watch_template").unwrap();
    second_table.ID = 0;
    second_table.Name = NewCIStr("ttl_watch_scan_target");
    second_table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain
        .ddl_create_table("test", second_table, false)
        .unwrap();
    scan_sender.send(b"scan-finished".to_vec()).unwrap();
    let scan_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let timers = sql
            .execute(
                "SELECT TIMER_KEY FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
                &[],
            )
            .unwrap();
        if timers.len() == 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < scan_deadline,
            "scan notification did not wake the manager before its 60s ticker"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    command_sender
        .send(br#"{"request_id":"req-sql","cmd_type":"trigger_ttl_job","data":{"db_name":"test","table_name":"ttl_watch_target"}}"#.to_vec())
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some((request_id, response)) = transport.responses.lock().unwrap().first() {
            assert_eq!(request_id, "req-sql");
            let response = response.as_ref().expect("manual TTL response");
            let job_id = response["table_result"][0]["job_id"]
                .as_str()
                .expect("manual job ID");
            let history = sql
                .execute(
                    "SELECT job_id FROM mysql.tidb_ttl_job_history WHERE job_id=%?",
                    &[Datum::Text(job_id.into())],
                )
                .unwrap();
            assert_eq!(
                history.len(),
                1,
                "manual trigger must submit a real SQL job"
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            let timer = sql.execute(
                "SELECT EVENT_STATUS,EVENT_ID,TIMER_EXT FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
                &[],
            ).unwrap();
            let history = sql.execute(
                "SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_watch_target'",
                &[],
            ).unwrap();
            panic!(
                "watch command timed out; subscriptions={}; timer={timer:?}; history={history:?}",
                transport.command_subscriptions.load(Ordering::SeqCst)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    domain.close();
    assert!(command_sender.send(b"after-close".to_vec()).is_err());
    assert!(scan_sender.send(b"after-close".to_vec()).is_err());
}

#[test]
fn go_merge_43_ttl_runtime_hook_submits_and_closes_event() {
    let (domain, setup) = CreateAnalyzeSession().expect("SQL domain");
    setup
        .execute("CREATE TABLE ttl_hook_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .expect("create metadata template");
    let (_, mut table) = domain.stats_table("test", "ttl_hook_template").unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_hook_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain.ddl_create_table("test", table, false).unwrap();
    setup
        .execute("INSERT INTO ttl_hook_target VALUES (1, '1970-01-01 00:00:01')")
        .unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let schedules = super::ttl_metadata::collect_ttl_schedules(domain.info_schema().as_ref(), now)
        .expect("TTL schedule");
    assert_eq!(schedules.len(), 1);
    let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    sync_ttl_timers(&mut sql, &schedules, now).expect("persist timer");
    let initial_watermark = sql
        .execute(
            "SELECT WATERMARK FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
            &[],
        )
        .unwrap()[0][0]
        .clone();
    assert!(start_domain_ttl_job_manager(&domain).unwrap());
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let rows = sql.execute(
            "SELECT EVENT_STATUS,EVENT_ID,WATERMARK,SUMMARY_DATA FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'",
            &[],
        ).unwrap();
        let history = sql.execute(
            "SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_hook_target'",
            &[],
        ).unwrap();
        if let (Some(timer), Some(job)) = (rows.first(), history.first()) {
            if job.get(1) == Some(&Datum::Text("finished".into()))
                && timer[0] == Datum::Text("IDLE".into())
                && !matches!(&timer[3], Datum::Text(value) if value == "<nil>")
            {
                let Datum::Text(summary) = &timer[3] else {
                    panic!("timer summary missing: {timer:?}")
                };
                let summary: serde_json::Value =
                    serde_json::from_str(summary).expect("timer summary JSON");
                assert_eq!(timer[0], Datum::Text("IDLE".into()));
                assert_eq!(timer[1], Datum::Text("".into()));
                let Datum::Text(job_id) = &job[0] else {
                    panic!("job ID missing")
                };
                assert_eq!(summary["last_job_request_id"], job_id.as_str());
                assert_ne!(
                    timer[2], initial_watermark,
                    "watermark must advance to the event start"
                );
                assert_eq!(history.len(), 1, "one timer event must submit one job");
                let remaining = setup.execute("SELECT id FROM ttl_hook_target").unwrap();
                assert_eq!(remaining.into_iter().flat_map(|set| set.rows).count(), 0);
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "TTL hook did not close an event"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    domain.close();
}

#[test]
fn go_merge_43_ttl_two_managers_single_event_and_takeover() {
    let (domain, setup) = CreateAnalyzeSession().expect("shared SQL domain");
    setup
        .execute("CREATE TABLE ttl_two_manager_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .unwrap();
    let (_, mut table) = domain
        .stats_table("test", "ttl_two_manager_template")
        .unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_two_manager_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    domain.ddl_create_table("test", table, false).unwrap();
    setup
        .execute("INSERT INTO ttl_two_manager_target VALUES (1, '1970-01-01 00:00:01')")
        .unwrap();
    let physical = collect_physical_ttl_tables(domain.info_schema().as_ref(), 200_000)
        .unwrap()
        .into_iter()
        .find(|item| item.table == "ttl_two_manager_target")
        .unwrap();
    let event_id = "two-manager-event";
    let mut workers = Vec::new();
    for owner in ["manager-a", "manager-b"] {
        let domain = Arc::clone(&domain);
        let table_id = physical.table_id;
        let physical_id = physical.physical_id;
        workers.push(std::thread::spawn(move || {
            run_ttl_event(
                &domain,
                owner,
                200_000,
                table_id,
                physical_id,
                event_id,
                || false,
            )
        }));
    }
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(
        results.iter().all(|result| match result {
            Ok(_) => true,
            Err(error) => error.contains("NOWAIT") && error.contains("3572"),
        }),
        "unexpected claim result: {results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .map(|result| result.claimed)
            .sum::<usize>(),
        1
    );
    let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
    let history = sql.execute(
        "SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_two_manager_target'",
        &[],
    ).unwrap();
    assert_eq!(
        history,
        vec![vec![
            Datum::Text(event_id.into()),
            Datum::Text("finished".into())
        ]]
    );
    assert!(
        sql.execute("SELECT id FROM ttl_two_manager_target", &[])
            .unwrap()
            .is_empty()
    );

    assert!(
        PersistentJobStore::start_job(
            &mut sql,
            &physical,
            "failed-manager",
            "takeover-job",
            200_000,
            None,
        )
        .unwrap()
    );
    sql.execute(
        "UPDATE mysql.tidb_ttl_task SET state=%? WHERE job_id=%? AND scan_id=0",
        &[
            Datum::Text(
                r#"{"cursor":["2"],"total_rows":1,"success_rows":1,"error_rows":0}"#.into(),
            ),
            Datum::Text("takeover-job".into()),
        ],
    )
    .unwrap();
    setup.execute("INSERT INTO ttl_two_manager_target VALUES (2, '1970-01-01 00:00:01'), (3, '1970-01-01 00:00:01')").unwrap();
    let schedules =
        super::ttl_metadata::collect_ttl_schedules(domain.info_schema().as_ref(), 200_300).unwrap();
    sync_ttl_timers(&mut sql, &schedules, 200_300).unwrap();
    let mut workers = Vec::new();
    for owner in ["manager-a", "manager-b"] {
        let domain = Arc::clone(&domain);
        workers.push(std::thread::spawn(move || {
            run_ttl_tick(&domain, owner, 200_300, || false)
        }));
    }
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(
        results.iter().all(|result| match result {
            Ok(_) => true,
            Err(error) => error.contains("NOWAIT") && error.contains("3572"),
        }),
        "unexpected takeover result: {results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .map(|result| result.resumed)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        sql.execute("SELECT id FROM ttl_two_manager_target ORDER BY id", &[])
            .unwrap(),
        vec![vec![Datum::Text("2".into())]]
    );
    let status = sql.execute(
        "SELECT last_job_id,last_job_summary FROM mysql.tidb_ttl_table_status WHERE table_id=%?",
        &[Datum::Integer(physical.physical_id)],
    ).unwrap();
    assert_eq!(status[0][0], Datum::Text("takeover-job".into()));
    let Datum::Text(summary) = &status[0][1] else {
        panic!("missing takeover summary")
    };
    assert!(summary.contains("\"total_rows\":2"), "{summary}");
    domain.close();
}

#[test]
fn go_merge_43_ttl_two_domains_real_etcd_timer_event() {
    struct RestoreDeleteRate(i64);
    impl Drop for RestoreDeleteRate {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::TTLDeleteRateLimit.Store(self.0);
        }
    }
    let _restore_delete_rate =
        RestoreDeleteRate(astersql_sessionctx_vardef::TTLDeleteRateLimit.Load());
    astersql_sessionctx_vardef::TTLDeleteRateLimit.Store(0);
    let Ok(endpoint) = std::env::var("ASTERSQL_TTL_ETCD_ENDPOINT") else {
        return;
    };
    let etcd = astersql_domain_serverinfo::RealEtcdClient::connect(vec![endpoint], None)
        .expect("connect real PD etcd");
    let (first, setup) = CreateAnalyzeSession().expect("first SQL Domain");
    setup
        .execute("CREATE TABLE ttl_etcd_template (id INT PRIMARY KEY, expire_at DATETIME)")
        .unwrap();
    let (_, mut table) = first.stats_table("test", "ttl_etcd_template").unwrap();
    table.ID = 0;
    table.Name = NewCIStr("ttl_etcd_target");
    table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    first.ddl_create_table("test", table, false).unwrap();
    let (_, initial_meta) = first.stats_table("test", "ttl_etcd_target").unwrap();
    first.persist_stats_meta(&[initial_meta.ID]).unwrap();
    setup
        .execute("INSERT INTO ttl_etcd_target VALUES (1, '1970-01-01 00:00:01')")
        .unwrap();

    let mut config = DomainConfig::default();
    config.schema_lease = Duration::ZERO;
    config.stats_lease = Duration::ZERO;
    let second = Arc::new(Domain::new_with_storage_handle(
        first.storage_handle(),
        Arc::new(KvInfoSchemaLoader::new()),
        config,
    ));
    second.init().unwrap();
    second.reload().unwrap();
    second.initialize_stats().unwrap();
    assert!(second.stats_table("test", "ttl_etcd_target").is_some());
    assert!(
        second
            .stats_handle()
            .lock()
            .unwrap()
            .stats_meta(initial_meta.ID)
            .is_some()
    );
    let mut second_sql = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&second)));
    assert_eq!(
        second_sql
            .execute("SELECT id FROM ttl_etcd_target", &[])
            .unwrap(),
        vec![vec![Datum::Text("1".into())]]
    );

    let first_transport = Arc::new(EtcdTtlWatchTransport::new(etcd.raw_client(), String::new()));
    let second_transport = Arc::new(EtcdTtlWatchTransport::new(etcd.raw_client(), String::new()));
    assert!(
        start_domain_ttl_job_manager_with_interval(
            &first,
            Some(first_transport),
            Duration::from_millis(500),
        )
        .unwrap()
    );
    assert!(
        start_domain_ttl_job_manager_with_interval(
            &second,
            Some(second_transport),
            Duration::from_millis(500),
        )
        .unwrap()
    );

    let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&first)));
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    loop {
        let history = sql.execute("SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_etcd_target'", &[]).unwrap();
        let timers = sql.execute("SELECT EVENT_STATUS,EVENT_ID,SUMMARY_DATA FROM mysql.tidb_timers WHERE HOOK_CLASS='tidb.ttl'", &[]).unwrap();
        if history.len() == 1
            && history[0][1] == Datum::Text("finished".into())
            && timers.len() == 1
            && timers[0][0] == Datum::Text("IDLE".into())
            && timers[0][1] == Datum::Text("".into())
        {
            let Datum::Text(job_id) = &history[0][0] else {
                panic!("missing job ID")
            };
            let Datum::Text(summary) = &timers[0][2] else {
                panic!("missing timer summary")
            };
            let summary: serde_json::Value = serde_json::from_str(summary).unwrap();
            assert_eq!(summary["last_job_request_id"], job_id.as_str());
            assert!(
                sql.execute("SELECT id FROM ttl_etcd_target", &[])
                    .unwrap()
                    .is_empty()
            );
            break;
        }
        if std::time::Instant::now() >= deadline {
            let status = sql.execute("SELECT current_job_id,current_job_owner_id,current_job_owner_hb_time FROM mysql.tidb_ttl_table_status", &[]).unwrap();
            let tasks = sql
                .execute(
                    "SELECT job_id,scan_id,status,state FROM mysql.tidb_ttl_task",
                    &[],
                )
                .unwrap();
            panic!(
                "two Domain timer event did not finish: history={history:?}, timers={timers:?}, status={status:?}, tasks={tasks:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    second.close();
    assert!(second.is_closed());
    astersql_sessionctx_vardef::TTLDeleteRateLimit.Store(1);
    let (_, mut failover_table) = first.stats_table("test", "ttl_etcd_template").unwrap();
    failover_table.ID = 0;
    failover_table.Name = NewCIStr("ttl_etcd_failover");
    failover_table.TTLInfo = Some(TTLInfo {
        ColumnName: NewCIStr("expire_at"),
        IntervalExprStr: "1".into(),
        IntervalTimeUnit: TimeUnitType::Day as i32,
        Enable: true,
        JobInterval: "24h".into(),
    });
    first
        .ddl_create_table("test", failover_table, false)
        .unwrap();
    let (_, failover_meta) = first.stats_table("test", "ttl_etcd_failover").unwrap();
    first.persist_stats_meta(&[failover_meta.ID]).unwrap();
    let failover_key =
        astersql_ttl_ttlworker::timer_sync::timer_key(failover_meta.ID, failover_meta.ID);
    setup.execute("INSERT INTO ttl_etcd_failover VALUES (1, '1970-01-01 00:00:01'), (2, '1970-01-01 00:00:01'), (3, '1970-01-01 00:00:01'), (4, '1970-01-01 00:00:01')").unwrap();
    let event_id = loop {
        let history = sql.execute("SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_etcd_failover'", &[]).unwrap();
        let timer = sql
            .execute(
                "SELECT EVENT_ID,EVENT_STATUS FROM mysql.tidb_timers WHERE TIMER_KEY=%?",
                &[Datum::Text(failover_key.clone())],
            )
            .unwrap();
        if history.len() == 1 && history[0][1] == Datum::Text("running".into()) {
            let Datum::Text(event_id) = &history[0][0] else {
                panic!("missing failover event ID")
            };
            assert!(!event_id.is_empty());
            assert_eq!(timer[0][0], Datum::Text(event_id.clone()));
            break event_id.clone();
        }
        assert!(
            std::time::Instant::now() < deadline + Duration::from_secs(30),
            "first manager did not start failover job: {history:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let original_expire = sql.execute(
        "SELECT current_job_ttl_expire FROM mysql.tidb_ttl_table_status WHERE current_job_id=%?",
        &[Datum::Text(event_id.clone())],
    ).unwrap()[0][0].clone();
    let replacement = Arc::new(Domain::new_with_storage_handle(
        first.storage_handle(),
        Arc::new(KvInfoSchemaLoader::new()),
        {
            let mut config = DomainConfig::default();
            config.schema_lease = Duration::ZERO;
            config.stats_lease = Duration::ZERO;
            config
        },
    ));
    replacement.init().unwrap();
    replacement.reload().unwrap();
    replacement.initialize_stats().unwrap();
    assert!(
        replacement
            .stats_table("test", "ttl_etcd_failover")
            .is_some()
    );
    assert!(
        replacement
            .stats_handle()
            .lock()
            .unwrap()
            .stats_meta(failover_meta.ID)
            .is_some()
    );
    first.close();
    astersql_sessionctx_vardef::TTLDeleteRateLimit.Store(0);
    let mut replacement_sql =
        TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&replacement)));
    replacement_sql.execute("UPDATE mysql.tidb_ttl_table_status SET current_job_owner_hb_time='1970-01-01 00:00:01' WHERE current_job_id=%?", &[Datum::Text(event_id.clone())]).unwrap();
    let replacement_transport =
        Arc::new(EtcdTtlWatchTransport::new(etcd.raw_client(), String::new()));
    assert!(
        start_domain_ttl_job_manager_with_interval(
            &replacement,
            Some(replacement_transport),
            Duration::from_millis(500),
        )
        .unwrap()
    );
    let takeover_deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let history = replacement_sql.execute("SELECT job_id,status FROM mysql.tidb_ttl_job_history WHERE table_name='ttl_etcd_failover'", &[]).unwrap();
        let timer = replacement_sql.execute("SELECT EVENT_STATUS,EVENT_ID,SUMMARY_DATA FROM mysql.tidb_timers WHERE TIMER_KEY=%?", &[Datum::Text(failover_key.clone())]).unwrap();
        if history.len() == 1
            && history[0][1] == Datum::Text("finished".into())
            && timer.len() == 1
            && timer[0][0] == Datum::Text("IDLE".into())
        {
            assert_eq!(history[0][0], Datum::Text(event_id.clone()));
            assert_eq!(timer[0][0], Datum::Text("IDLE".into()));
            assert_eq!(timer[0][1], Datum::Text("".into()));
            let Datum::Text(summary) = &timer[0][2] else {
                panic!("missing failover summary")
            };
            let summary: serde_json::Value = serde_json::from_str(summary).unwrap();
            assert_eq!(summary["last_job_request_id"], event_id);
            let persisted = replacement_sql.execute(
                "SELECT last_job_ttl_expire FROM mysql.tidb_ttl_table_status WHERE last_job_id=%?",
                &[Datum::Text(event_id.clone())],
            ).unwrap();
            assert_eq!(
                persisted[0][0], original_expire,
                "takeover must preserve the original expiration watermark"
            );
            assert!(
                replacement_sql
                    .execute("SELECT id FROM ttl_etcd_failover", &[])
                    .unwrap()
                    .is_empty()
            );
            break;
        }
        if std::time::Instant::now() >= takeover_deadline {
            let status = replacement_sql.execute("SELECT current_job_id,current_job_owner_id,current_job_owner_hb_time FROM mysql.tidb_ttl_table_status", &[]).unwrap();
            let tasks = replacement_sql
                .execute(
                    "SELECT job_id,scan_id,status,state FROM mysql.tidb_ttl_task",
                    &[],
                )
                .unwrap();
            panic!(
                "replacement did not finish event {event_id}: history={history:?}, timer={timer:?}, status={status:?}, tasks={tasks:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    replacement.close();
    assert!(first.is_closed() && second.is_closed() && replacement.is_closed());
}

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
            None,
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
    assert_eq!(
        event_rows[0][1],
        Datum::Text("<nil>".into()),
        "only the timer Hook may close and summarize an event"
    );
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
        vec![vec!["IDLE".to_owned()]]
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
            None,
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

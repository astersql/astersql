// Copyright 2026 AsterSQL.

//! One scheduling pass of the SQL-backed TTL job manager. A Domain worker
//! drives this repeatedly after bootstrap; this module owns the real session,
//! scan, delete, retry, and durable completion path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_domain::Domain;
use astersql_timer_api::{
    Context as TimerContext, NewDefaultTimerClient, NewOptionalVal, TimerClient, TimerCond,
    TimerStore,
};
use astersql_timer_runtime::runtime::{NewTimerRuntimeBuilder, TimerGroupRuntime};
use astersql_timer_tablestore::NewTableTimerStore;
use astersql_ttl_ttlworker::del::{DeleteRateLimiter, DeleteRetryBuffer, DeleteTask};
use astersql_ttl_ttlworker::job_manager::TtlSummary;
use astersql_ttl_ttlworker::persistent::PersistentJobStore;
use astersql_ttl_ttlworker::scan::{TaskTerminateReason, TtlScanTask, TtlStatistics};
use astersql_ttl_ttlworker::session::{Datum, SessionError, WorkerSession};
use astersql_util_timeutil::time_zone::WithinDayTimePeriod;

use super::ConcreteSession;
use super::ttl_metadata::{collect_ttl_schedules, split_ttl_scan_ranges};
use super::ttl_timer::{SqlTtlTimerHook, sync_ttl_timers};
use super::ttl_timer_store::new_ttl_timer_session_pool;
use super::ttl_worker_session::TtlWorkerSqlSession;

fn trigger_ttl_command(
    domain: &Arc<Domain>,
    store: TimerStore,
    db_name: &str,
    table_name: &str,
    stopped: &AtomicBool,
) -> Result<serde_json::Value, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if !astersql_sessionctx_vardef::EnableTTLJob.Load() {
        return Err("tidb_ttl_job_enable is disabled".into());
    }
    if !within_ttl_window(
        now,
        &astersql_sessionctx_vardef::TTLJobScheduleWindowStartTime.Load(),
        &astersql_sessionctx_vardef::TTLJobScheduleWindowEndTime.Load(),
    )? {
        return Err("not in TTL job window".into());
    }
    let schedules = collect_ttl_schedules(domain.info_schema().as_ref(), now)?;
    let selected: Vec<_> = schedules
        .iter()
        .filter(|schedule| {
            schedule.table.schema.eq_ignore_ascii_case(db_name)
                && schedule.table.table.eq_ignore_ascii_case(table_name)
        })
        .collect();
    if selected.is_empty() {
        return Err(format!("table {db_name}.{table_name} not exists"));
    }
    let client = NewDefaultTimerClient(store);
    let context = TimerContext::background();
    let mut results = Vec::with_capacity(selected.len());
    let mut pending = Vec::new();
    for schedule in selected {
        let table = &schedule.table;
        let mut result = serde_json::json!({
            "table_id": table.physical_id,
            "db_name": db_name,
            "table_name": table_name,
        });
        if let Some(partition_name) = &table.partition_name {
            result["partition_name"] = partition_name.clone().into();
        }
        let outcome = client
            .GetTimerByKey(
                &context,
                &astersql_ttl_ttlworker::timer_sync::timer_key(table.table_id, table.physical_id),
            )
            .and_then(|timer| {
                client
                    .ManualTriggerEvent(&context, &timer.ID)
                    .map(|request_id| (timer.ID, request_id))
            });
        match outcome {
            Ok((timer_id, request_id)) => pending.push((results.len(), timer_id, request_id)),
            Err(error) => result["error_message"] = error.to_string().into(),
        }
        results.push(result);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let mut session = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
    while !pending.is_empty() && !stopped.load(Ordering::Acquire) && !domain.is_closed() {
        if std::time::Instant::now() >= deadline {
            break;
        }
        pending.retain(|(index, timer_id, request_id)| {
            let timer = match client.GetTimerByID(&context, timer_id) {
                Ok(timer) => timer,
                Err(error) => {
                    results[*index]["error_message"] = error.to_string().into();
                    return false;
                }
            };
            if timer.ManualRequest.ManualRequestID != *request_id {
                results[*index]["error_message"] = "manual request not found".into();
                return false;
            }
            if !timer.ManualRequest.ManualProcessed {
                return true;
            }
            let job_id = &timer.ManualRequest.ManualEventID;
            if job_id.is_empty() {
                results[*index]["error_message"] = "manual request cancelled".into();
                return false;
            }
            match session.execute(
                "SELECT 1 FROM mysql.tidb_ttl_job_history WHERE job_id=%?",
                &[Datum::Text(job_id.clone())],
            ) {
                Ok(rows) if !rows.is_empty() => {
                    results[*index]["job_id"] = job_id.clone().into();
                    false
                }
                Ok(_) => true,
                Err(error) => {
                    results[*index]["error_message"] =
                        format!("read TTL job history: {error:?}").into();
                    false
                }
            }
        });
        if !pending.is_empty() {
            std::thread::park_timeout(Duration::from_millis(200));
        }
    }
    for (index, _, _) in pending {
        results[index]["error_message"] = "timeout".into();
    }
    if results.iter().all(|result| result.get("job_id").is_none()) {
        return Err(results[0]["error_message"]
            .as_str()
            .unwrap_or("TTL manual trigger failed")
            .to_owned());
    }
    Ok(serde_json::json!({ "table_result": results }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TtlWatchKind {
    Command,
    Scan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum TtlWatchEvent {
    Command {
        request_id: String,
        db_name: String,
        table_name: String,
    },
    Scan,
}

/// A watch ends when its receiver disconnects. The caller then opens a fresh
/// subscription against the same transport, as Go's job loop does.
pub(super) trait TtlWatchTransport: Send + Sync + 'static {
    fn ttl_owner(&self, _id: &str) -> Option<Arc<dyn astersql_owner::Manager>> {
        None
    }

    fn timer_notifier(&self) -> Option<Arc<dyn astersql_timer_tablestore::EtcdClient>> {
        None
    }
    fn watch(
        &self,
        kind: TtlWatchKind,
        stopped: Arc<AtomicBool>,
    ) -> Result<mpsc::Receiver<Vec<u8>>, String>;
    fn take_command(&self, request_id: &str) -> Result<bool, String>;
    fn response_command(
        &self,
        request_id: &str,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), String>;
}

fn decode_ttl_command(value: &[u8]) -> Option<TtlWatchEvent> {
    let request: serde_json::Value = serde_json::from_slice(value).ok()?;
    if request.get("cmd_type")?.as_str()? != "trigger_ttl_job" {
        return None;
    }
    Some(TtlWatchEvent::Command {
        request_id: request.get("request_id")?.as_str()?.to_owned(),
        db_name: request.get("data")?.get("db_name")?.as_str()?.to_owned(),
        table_name: request.get("data")?.get("table_name")?.as_str()?.to_owned(),
    })
}

pub(super) struct TtlWatchRuntime {
    stopped: Arc<AtomicBool>,
    receiver: mpsc::Receiver<TtlWatchEvent>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl TtlWatchRuntime {
    pub(super) fn start(
        transport: Arc<dyn TtlWatchTransport>,
        manager_thread: std::thread::Thread,
    ) -> Self {
        let stopped = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let workers = [TtlWatchKind::Command, TtlWatchKind::Scan]
            .into_iter()
            .map(|kind| {
                let transport = Arc::clone(&transport);
                let stopped = Arc::clone(&stopped);
                let sender = sender.clone();
                let manager_thread = manager_thread.clone();
                std::thread::Builder::new()
                    .name(format!("ttl-{kind:?}-watch"))
                    .spawn(move || {
                        while !stopped.load(Ordering::Acquire) {
                            match transport.watch(kind, Arc::clone(&stopped)) {
                                Ok(watch) => loop {
                                    if stopped.load(Ordering::Acquire) {
                                        return;
                                    }
                                    match watch.recv_timeout(Duration::from_millis(100)) {
                                        Ok(bytes) => {
                                            let event = match kind {
                                                TtlWatchKind::Command => decode_ttl_command(&bytes),
                                                TtlWatchKind::Scan => Some(TtlWatchEvent::Scan),
                                            };
                                            if let Some(event) = event {
                                                if sender.send(event).is_err() {
                                                    return;
                                                }
                                                manager_thread.unpark();
                                            }
                                        }
                                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                    }
                                },
                                Err(error) => {
                                    super::BgLogger().log(
                                        super::LogLevel::Warn,
                                        "TTL watcher subscription failed",
                                        [super::LogField::String("error".into(), error)],
                                    );
                                }
                            }
                            // A failed or closed subscription must not spin while the
                            // remote transport is unavailable.
                            if !stopped.load(Ordering::Acquire) {
                                std::thread::park_timeout(Duration::from_millis(100));
                            }
                        }
                    })
                    .expect("start TTL watch worker")
            })
            .collect();
        Self {
            stopped,
            receiver,
            workers,
        }
    }

    pub(super) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<TtlWatchEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub(super) fn try_recv(&self) -> Result<TtlWatchEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    pub(super) fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

impl Drop for TtlWatchRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) struct EtcdTtlWatchTransport {
    client: etcd_client::Client,
    namespace: String,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl EtcdTtlWatchTransport {
    pub(super) fn new(client: etcd_client::Client, namespace: String) -> Self {
        Self {
            client,
            namespace,
            workers: Mutex::new(Vec::new()),
        }
    }
}

impl TtlWatchTransport for EtcdTtlWatchTransport {
    fn ttl_owner(&self, id: &str) -> Option<Arc<dyn astersql_owner::Manager>> {
        if cfg!(test) {
            return None;
        }
        Some(astersql_owner::NewOwnerManager(
            astersql_owner::Context::new(),
            self.client.clone(),
            "ttl_job_manager",
            id,
            format!("{}/tidb/ttl_job_manager/leader", self.namespace),
        ))
    }

    fn timer_notifier(&self) -> Option<Arc<dyn astersql_timer_tablestore::EtcdClient>> {
        Some(Arc::new(super::ttl_timer_etcd::RealTimerEtcdClient::new(
            self.client.clone(),
            self.namespace.clone(),
        )))
    }
    fn watch(
        &self,
        kind: TtlWatchKind,
        stopped: Arc<AtomicBool>,
    ) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        let mut client = self.client.clone();
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let key = format!(
            "{}{}",
            self.namespace,
            match kind {
                TtlWatchKind::Command => "/tidb/ttl/cmd/req/",
                TtlWatchKind::Scan => "/tidb/ttl/notification/scan",
            }
        );
        let worker = std::thread::Builder::new()
            .name(format!("ttl-etcd-{kind:?}-watch"))
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender.send(Err(format!("start TTL etcd watch: {error}")));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let options = match kind {
                        TtlWatchKind::Command => {
                            Some(etcd_client::WatchOptions::new().with_prefix())
                        }
                        TtlWatchKind::Scan => None,
                    };
                    let mut stream = match tokio::time::timeout(
                        Duration::from_secs(5),
                        client.watch(key, options),
                    )
                    .await
                    {
                        Ok(Ok(watch)) => watch,
                        other => {
                            let _ = ready_sender
                                .send(Err(format!("subscribe TTL etcd watch: {other:?}")));
                            return;
                        }
                    };
                    let _ = ready_sender.send(Ok(()));
                    while !stopped.load(Ordering::Acquire) {
                        match tokio::time::timeout(Duration::from_millis(100), stream.message())
                            .await
                        {
                            Ok(Ok(Some(response))) if !response.canceled() => {
                                for event in response.events() {
                                    if event.event_type() == etcd_client::EventType::Put {
                                        if let Some(value) = event.kv() {
                                            if sender.send(value.value().to_vec()).is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(Ok(Some(_))) | Ok(Ok(None)) | Ok(Err(_)) => return,
                            Err(_) => {}
                        }
                    }
                });
            })
            .map_err(|error| format!("start TTL etcd watch thread: {error}"))?;
        let mut workers = self.workers.lock().expect("TTL etcd watch lock poisoned");
        let mut active = Vec::new();
        for worker in workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                active.push(worker);
            }
        }
        active.push(worker);
        *workers = active;
        drop(workers);
        ready_receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| format!("wait for TTL etcd watch: {error}"))??;
        Ok(receiver)
    }

    fn take_command(&self, request_id: &str) -> Result<bool, String> {
        let key = format!("{}/tidb/ttl/cmd/req/{request_id}", self.namespace);
        let txn = etcd_client::Txn::new()
            .when([etcd_client::Compare::create_revision(
                key.clone(),
                etcd_client::CompareOp::Greater,
                0,
            )])
            .and_then([etcd_client::TxnOp::delete(key, None)]);
        let mut client = self.client.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("start TTL command runtime: {error}"))?;
        runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(5), client.txn(txn)).await })
            .map_err(|error| format!("take TTL command {request_id} timed out: {error}"))?
            .map(|response| response.succeeded())
            .map_err(|error| format!("take TTL command {request_id}: {error}"))
    }

    fn response_command(
        &self,
        request_id: &str,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        let (data, error_message) = match result {
            Ok(data) => (data, String::new()),
            Err(error) => (serde_json::Value::Null, error),
        };
        let value = serde_json::json!({
            "request_id": request_id,
            "error_message": error_message,
            "data": data,
        });
        let key = format!("{}/tidb/ttl/cmd/resp/{request_id}", self.namespace);
        let mut client = self.client.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("start TTL response runtime: {error}"))?;
        runtime
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    let lease = client.lease_grant(180, None).await?;
                    client
                        .put(
                            key,
                            value.to_string(),
                            Some(etcd_client::PutOptions::new().with_lease(lease.id())),
                        )
                        .await?;
                    Ok::<(), etcd_client::Error>(())
                })
                .await
            })
            .map_err(|error| format!("respond TTL command {request_id} timed out: {error}"))?
            .map_err(|error| format!("respond TTL command {request_id}: {error}"))
    }
}

impl Drop for EtcdTtlWatchTransport {
    fn drop(&mut self) {
        for worker in self
            .workers
            .lock()
            .expect("TTL etcd watch lock poisoned")
            .drain(..)
        {
            let _ = worker.join();
        }
    }
}

struct TtlOwnerListener;
impl astersql_owner::manager::Listener for TtlOwnerListener {
    fn OnRetireOwner(&self) {}
    fn OnBecomeOwner(&self) {
        super::BgLogger().log(
            super::LogLevel::Info,
            "leader change of TTL job manager service, this node become owner",
            [],
        );
    }
}

/// The runtime keeps the campaign alive and releases its lease with the loop.
struct TtlElection {
    runtime: tokio::runtime::Runtime,
    manager: Arc<dyn astersql_owner::Manager>,
}
impl TtlElection {
    fn start(manager: Arc<dyn astersql_owner::Manager>) -> Result<Self, String> {
        let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        runtime.block_on(manager.SetListener(Arc::new(TtlOwnerListener)));
        if let Err(error) = runtime.block_on(manager.CampaignOwner(&[5])) {
            super::BgLogger().log(
                super::LogLevel::Error,
                "failed to campaign ttl job manager owner",
                [super::LogField::String("error".into(), error.to_string())],
            );
        }
        Ok(Self { runtime, manager })
    }
}
impl Drop for TtlElection {
    fn drop(&mut self) {
        self.runtime.block_on(self.manager.Close());
    }
}

struct DomainTtlTimerRuntime {
    runtime: TimerGroupRuntime,
    store: TimerStore,
    pool: Arc<astersql_session_syssession::AdvancedSessionPool>,
}

#[derive(Default)]
struct TtlCommandWorkers(Vec<std::thread::JoinHandle<()>>);

impl TtlCommandWorkers {
    fn reap(&mut self) {
        let mut remaining = Vec::new();
        for worker in self.0.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                remaining.push(worker);
            }
        }
        self.0 = remaining;
    }
}

impl Drop for TtlCommandWorkers {
    fn drop(&mut self) {
        for worker in self.0.drain(..) {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

impl Drop for DomainTtlTimerRuntime {
    fn drop(&mut self) {
        self.runtime.Stop();
        self.store.Close();
        self.pool.Close();
    }
}

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// Heartbeats use their own SQL session so a long scan statement or delete
/// rate wait cannot starve the durable owner lease.
pub(super) struct JobHeartbeat {
    wake: Arc<(Mutex<bool>, Condvar)>,
    lost: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl JobHeartbeat {
    pub(super) fn start(
        domain: Arc<Domain>,
        table_id: i64,
        job_id: String,
        owner_id: String,
        interval: Duration,
    ) -> Self {
        let wake = Arc::new((Mutex::new(false), Condvar::new()));
        let lost = Arc::new(AtomicBool::new(false));
        let worker_wake = Arc::clone(&wake);
        let worker_lost = Arc::clone(&lost);
        let worker = std::thread::Builder::new()
            .name("ttl-job-heartbeat".into())
            .spawn(move || {
                let mut session = TtlWorkerSqlSession::new(ConcreteSession::new(domain));
                loop {
                    let (lock, notified) = &*worker_wake;
                    let stopped = lock.lock().expect("TTL heartbeat lock poisoned");
                    let (stopped, _) = notified
                        .wait_timeout_while(stopped, interval, |stopped| !*stopped)
                        .expect("TTL heartbeat wait poisoned");
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    match PersistentJobStore::heartbeat(
                        &mut session,
                        table_id,
                        &job_id,
                        &owner_id,
                        now,
                    ) {
                        Ok(true) => {}
                        Ok(false) | Err(_) => {
                            worker_lost.store(true, Ordering::Release);
                            return;
                        }
                    }
                }
            })
            .expect("create TTL heartbeat thread");
        Self {
            wake,
            lost,
            worker: Some(worker),
        }
    }

    pub(super) fn lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }

    pub(super) fn stop(&mut self) {
        let (lock, notified) = &*self.wake;
        *lock.lock().expect("TTL heartbeat lock poisoned") = true;
        notified.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for JobHeartbeat {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn within_ttl_window(now: u64, start: &str, end: &str) -> Result<bool, String> {
    let parse = |value: &str| {
        chrono::DateTime::parse_from_str(&format!("1970-01-01 {value}"), "%Y-%m-%d %H:%M %z")
            .map_err(|error| format!("invalid TTL schedule window {value}: {error}"))
    };
    let start = parse(start)?;
    let end = parse(end)?;
    let now = i64::try_from(now)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .ok_or_else(|| "TTL schedule time is outside supported range".to_owned())?;
    Ok(WithinDayTimePeriod(start, end, now))
}

fn scheduling_enabled(now: u64) -> Result<bool, String> {
    if !astersql_sessionctx_vardef::EnableTTLJob.Load() {
        return Ok(false);
    }
    within_ttl_window(
        now,
        &astersql_sessionctx_vardef::TTLJobScheduleWindowStartTime.Load(),
        &astersql_sessionctx_vardef::TTLJobScheduleWindowEndTime.Load(),
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TtlTickResult {
    pub tables: usize,
    pub claimed: usize,
    pub resumed: usize,
    pub finished: usize,
}

fn persisted_expire_time(session: &mut TtlWorkerSqlSession, job_id: &str) -> Result<u64, String> {
    let rows = session
        .execute(
            "SELECT expire_time FROM mysql.tidb_ttl_task WHERE job_id=%? AND scan_id=0",
            &[Datum::Text(job_id.into())],
        )
        .map_err(|error| format!("read TTL scan task {job_id}: {error:?}"))?;
    let Some(Datum::Text(expire)) = rows.first().and_then(|row| row.first()) else {
        return Err(format!("TTL scan task missing for job {job_id}"));
    };
    let parsed = chrono::NaiveDateTime::parse_from_str(expire, "%Y-%m-%d %H:%M:%S")
        .map_err(|error| format!("invalid TTL scan task expiry {expire}: {error}"))?;
    u64::try_from(parsed.and_utc().timestamp())
        .map_err(|_| format!("TTL scan task expiry out of range: {expire}"))
}

pub(super) struct PersistedScanRange {
    pub(super) scan_id: i64,
    pub(super) start: Option<Vec<Datum>>,
    pub(super) end: Option<Vec<Datum>>,
}

pub(super) fn persisted_scan_ranges(
    session: &mut TtlWorkerSqlSession,
    job_id: &str,
) -> Result<Vec<PersistedScanRange>, String> {
    let rows = session
        .execute(
            "SELECT scan_id,scan_range_start,scan_range_end FROM mysql.tidb_ttl_task WHERE job_id=%? ORDER BY scan_id",
            &[Datum::Text(job_id.into())],
        )
        .map_err(|error| format!("read TTL scan ranges: {error:?}"))?;
    rows.into_iter()
        .map(|row| {
            let Some(Datum::Text(scan_id)) = row.first() else {
                return Err("TTL scan ID is missing".into());
            };
            let scan_id = scan_id
                .parse::<i64>()
                .map_err(|error| format!("invalid TTL scan ID: {error}"))?;
            let decode = |index: usize| -> Result<Option<Vec<Datum>>, String> {
                let Some(Datum::Text(value)) = row.get(index) else {
                    return Err("TTL scan range is missing".into());
                };
                let bytes =
                    super::binary_runtime_bytes(value).unwrap_or_else(|| value.as_bytes().to_vec());
                let datums = astersql_ttl_cache::task::DecodeDatums(&bytes)?;
                let datums = datums
                    .into_iter()
                    .map(|datum| match datum {
                        astersql_ttl_cache::task::Datum::Null => Ok(Datum::Null),
                        astersql_ttl_cache::task::Datum::Int(value) => Ok(Datum::Integer(value)),
                        astersql_ttl_cache::task::Datum::UInt(value) => Ok(Datum::Unsigned(value)),
                        astersql_ttl_cache::task::Datum::Bytes(value) => Ok(Datum::Bytes(value)),
                        astersql_ttl_cache::task::Datum::String(value) => Ok(Datum::Text(value)),
                        other => Err(format!("unsupported TTL scan range datum: {other:?}")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((!datums.is_empty()).then_some(datums))
            };
            Ok(PersistedScanRange {
                scan_id,
                start: decode(1)?,
                end: decode(2)?,
            })
        })
        .collect()
}

#[derive(Default)]
struct PersistedTaskState {
    cursor: Option<Vec<Datum>>,
    total_rows: u64,
    success_rows: u64,
    error_rows: u64,
}

fn persisted_task_state(
    session: &mut TtlWorkerSqlSession,
    job_id: &str,
    scan_id: i64,
) -> Result<PersistedTaskState, String> {
    let rows = session
        .execute(
            "SELECT state FROM mysql.tidb_ttl_task WHERE job_id=%? AND scan_id=%?",
            &[Datum::Text(job_id.into()), Datum::Integer(scan_id)],
        )
        .map_err(|error| format!("read TTL scan cursor: {error:?}"))?;
    let Some(Datum::Text(state)) = rows.first().and_then(|row| row.first()) else {
        return Ok(PersistedTaskState::default());
    };
    if state.is_empty() || state.eq_ignore_ascii_case("null") || state == "<nil>" {
        return Ok(PersistedTaskState::default());
    }
    let state: serde_json::Value = serde_json::from_str(state)
        .map_err(|error| format!("invalid TTL task state {state:?}: {error}"))?;
    let cursor = state
        .get("cursor")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(|value| Datum::Text(value.into()))
                        .ok_or_else(|| "invalid TTL cursor cell".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let count = |name: &str| {
        state
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Ok(PersistedTaskState {
        cursor: cursor.filter(|cursor| !cursor.is_empty()),
        total_rows: count("total_rows"),
        success_rows: count("success_rows"),
        error_rows: count("error_rows"),
    })
}

fn checkpoint_cursor(
    session: &mut TtlWorkerSqlSession,
    table_id: i64,
    job_id: &str,
    scan_id: i64,
    owner_id: &str,
    cursor: &[Datum],
    statistics: &TtlStatistics,
) -> Result<(), SessionError> {
    let values = cursor
        .iter()
        .map(|datum| match datum {
            Datum::Text(value) => Ok(value.clone()),
            _ => Err(SessionError::Execute("TTL scan cursor must be text".into())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (total_rows, success_rows, error_rows) = statistics.snapshot();
    let state = serde_json::json!({
        "cursor": values,
        "total_rows": total_rows,
        "success_rows": success_rows,
        "error_rows": error_rows,
    })
    .to_string();
    session.execute("BEGIN PESSIMISTIC", &[])?;
    let result = (|| {
        if session
            .execute(
                "SELECT table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_id=%? AND current_job_owner_id=%? FOR UPDATE NOWAIT",
                &[
                    Datum::Integer(table_id),
                    Datum::Text(job_id.into()),
                    Datum::Text(owner_id.into()),
                ],
            )?
            .is_empty()
        {
            return Err(SessionError::Execute("TTL job ownership changed".into()));
        }
        if session
            .execute(
                "SELECT scan_id FROM mysql.tidb_ttl_task WHERE job_id=%? AND scan_id=%? FOR UPDATE NOWAIT",
                &[Datum::Text(job_id.into()), Datum::Integer(scan_id)],
            )?
            .is_empty()
        {
            return Err(SessionError::Execute("TTL scan task disappeared".into()));
        }
        session.execute(
            "UPDATE mysql.tidb_ttl_task SET state=%? WHERE job_id=%? AND scan_id=%?",
            &[
                Datum::Text(state),
                Datum::Text(job_id.into()),
                Datum::Integer(scan_id),
            ],
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            session.execute("COMMIT", &[])?;
            Ok(())
        }
        Err(error) => {
            session.execute("ROLLBACK", &[])?;
            Err(error)
        }
    }
}

/// Install the real TTL SQL loop after the canonical Domain has bootstrapped.
/// Domain owns its stop flag and joins the worker during `close`.
pub fn start_domain_ttl_job_manager(domain: &Arc<Domain>) -> Result<bool, String> {
    start_domain_ttl_job_manager_with_transport(domain, None)
}

pub(super) fn start_domain_ttl_job_manager_with_transport(
    domain: &Arc<Domain>,
    transport: Option<Arc<dyn TtlWatchTransport>>,
) -> Result<bool, String> {
    start_domain_ttl_job_manager_with_interval(domain, transport, Duration::from_secs(10))
}

pub(super) fn start_domain_ttl_job_manager_with_interval(
    domain: &Arc<Domain>,
    transport: Option<Arc<dyn TtlWatchTransport>>,
    interval: Duration,
) -> Result<bool, String> {
    static NEXT_OWNER_ID: AtomicU64 = AtomicU64::new(1);
    let owner_id = format!(
        "{}-{}-{}",
        domain.server_id(),
        std::process::id(),
        NEXT_OWNER_ID.fetch_add(1, Ordering::Relaxed)
    );
    if !domain.should_start_ttl_job_manager() {
        return Ok(false);
    }
    let election = if domain.ttl_external_workload_role().0 == astersql_config::RoleTTLTaskWorker {
        transport
            .as_ref()
            .and_then(|t| t.ttl_owner(&owner_id))
            .map(TtlElection::start)
            .transpose()?
    } else {
        None
    };
    let weak = Arc::downgrade(domain);
    let mut timer_runtime: Option<DomainTtlTimerRuntime> = None;
    let mut watcher: Option<TtlWatchRuntime> = None;
    let mut command_workers = TtlCommandWorkers::default();
    domain
        .start_ttl_job_manager(interval, move |stop| {
            let Some(domain) = weak.upgrade() else {
                return;
            };
            if watcher.is_none() {
                if let Some(transport) = &transport {
                    watcher = Some(TtlWatchRuntime::start(
                        Arc::clone(transport),
                        std::thread::current(),
                    ));
                }
            }
            let mut notifications = Vec::new();
            if let Some(watcher) = &watcher {
                while let Ok(event) = watcher.try_recv() {
                    notifications.push(event);
                }
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if election
                .as_ref()
                .is_some_and(|election| !election.manager.IsOwner())
            {
                timer_runtime.take();
                command_workers.reap();
                return;
            }
            let sync = (|| {
                let schedules = collect_ttl_schedules(domain.info_schema().as_ref(), now)?;
                let mut session =
                    TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(&domain)));
                sync_ttl_timers(&mut session, &schedules, now)?;
                if timer_runtime.is_none() {
                    let pool = new_ttl_timer_session_pool(Arc::clone(&domain), 8);
                    let store = NewTableTimerStore(
                        1,
                        pool.clone(),
                        "mysql",
                        "tidb_timers",
                        transport
                            .as_ref()
                            .and_then(|source| source.timer_notifier()),
                    );
                    let hook_domain = Arc::clone(&domain);
                    let hook_owner = owner_id.clone();
                    let runtime = NewTimerRuntimeBuilder("ttl".into(), store.clone())
                        .SetCond(Arc::new(TimerCond {
                            Key: NewOptionalVal(
                                astersql_ttl_ttlworker::timer_sync::TIMER_KEY_PREFIX.into(),
                            ),
                            KeyPrefix: true,
                            ..TimerCond::default()
                        }))
                        .RegisterHookFactory(
                            astersql_ttl_ttlworker::timer_sync::TIMER_HOOK_CLASS.into(),
                            Arc::new(move |_, client| {
                                Box::new(SqlTtlTimerHook::new(
                                    Arc::clone(&hook_domain),
                                    hook_owner.clone(),
                                    client,
                                ))
                            }),
                        )
                        .Build();
                    runtime.Start();
                    timer_runtime = Some(DomainTtlTimerRuntime {
                        runtime,
                        store,
                        pool,
                    });
                }
                Ok::<(), String>(())
            })();
            if let Err(error) = sync {
                super::BgLogger().log(
                    super::LogLevel::Error,
                    "TTL timer synchronization failed",
                    [super::LogField::String("error".into(), error)],
                );
            }
            domain.recycle_finished_ttl_jobs();
            command_workers.reap();
            for event in notifications {
                match event {
                    TtlWatchEvent::Scan => {
                        // The current SQL scan path runs to completion in the
                        // timer hook. Its notification wakes this manager for
                        // an immediate timer and metadata pass.
                    }
                    TtlWatchEvent::Command {
                        request_id,
                        db_name,
                        table_name,
                    } => {
                        let Some(transport) = &transport else {
                            continue;
                        };
                        match transport.take_command(&request_id) {
                            Ok(true) => {}
                            Ok(false) => continue,
                            Err(error) => {
                                super::BgLogger().log(
                                    super::LogLevel::Error,
                                    "take TTL command failed",
                                    [super::LogField::String("error".into(), error)],
                                );
                                continue;
                            }
                        }
                        let Some(runtime) = &timer_runtime else {
                            let _ = transport.response_command(
                                &request_id,
                                Err("TTL timer runtime unavailable".into()),
                            );
                            continue;
                        };
                        let transport = Arc::clone(transport);
                        let domain = Arc::clone(&domain);
                        let store = runtime.store.clone();
                        let stopped = watcher.as_ref().unwrap().stopped.clone();
                        command_workers.0.push(
                            std::thread::Builder::new()
                                .name("ttl-command-response".into())
                                .spawn(move || {
                                    let result = trigger_ttl_command(
                                        &domain,
                                        store,
                                        &db_name,
                                        &table_name,
                                        &stopped,
                                    );
                                    let _ = transport.response_command(&request_id, result);
                                })
                                .expect("start TTL command response worker"),
                        );
                    }
                }
            }
        })
        .map_err(|error| error.to_string())
}

struct ConfiguredDeleteRateLimiter<'a> {
    canceled: &'a dyn Fn() -> bool,
}

impl DeleteRateLimiter for ConfiguredDeleteRateLimiter<'_> {
    fn wait_delete_token(&mut self, rows: usize) -> Result<(), SessionError> {
        let limit = astersql_sessionctx_vardef::TTLDeleteRateLimit.Load();
        if limit > 0 {
            let mut remaining = Duration::from_secs_f64(rows as f64 / limit as f64);
            while !remaining.is_zero() {
                if (self.canceled)() {
                    return Err(SessionError::Execute("TTL job canceled".into()));
                }
                let step = remaining.min(Duration::from_millis(100));
                std::thread::sleep(step);
                remaining -= step;
            }
        }
        Ok(())
    }
}

pub fn run_ttl_tick(
    domain: &Arc<Domain>,
    owner_id: &str,
    now: u64,
    canceled: impl Fn() -> bool,
) -> Result<TtlTickResult, String> {
    run_ttl_tick_inner(domain, owner_id, now, canceled, None)
}

pub(super) fn run_ttl_event(
    domain: &Arc<Domain>,
    owner_id: &str,
    now: u64,
    table_id: i64,
    physical_id: i64,
    event_id: &str,
    canceled: impl Fn() -> bool,
) -> Result<TtlTickResult, String> {
    run_ttl_tick_inner(
        domain,
        owner_id,
        now,
        canceled,
        Some((table_id, physical_id, event_id)),
    )
}

fn run_ttl_tick_inner(
    domain: &Arc<Domain>,
    owner_id: &str,
    now: u64,
    canceled: impl Fn() -> bool,
    event: Option<(i64, i64, &str)>,
) -> Result<TtlTickResult, String> {
    let schedules = collect_ttl_schedules(domain.info_schema().as_ref(), now)?;
    let schedules: Vec<_> = schedules
        .into_iter()
        .filter(|schedule| {
            event.is_none_or(|(table_id, physical_id, _)| {
                schedule.table.table_id == table_id && schedule.table.physical_id == physical_id
            })
        })
        .collect();
    let mut result = TtlTickResult {
        tables: schedules.len(),
        ..TtlTickResult::default()
    };
    let mut coordinator = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
    if event.is_none() {
        sync_ttl_timers(&mut coordinator, &schedules, now)?;
    }
    if !scheduling_enabled(now)? {
        return Ok(result);
    }
    for schedule in schedules {
        if canceled() || !scheduling_enabled(now)? {
            break;
        }
        let table = schedule.table;
        let new_job_id = event.map(|(_, _, id)| id.to_owned()).unwrap_or_else(|| {
            format!(
                "ttl-{owner_id}-{}-{now}-{}",
                table.physical_id,
                NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed)
            )
        });
        let takeover = PersistentJobStore::takeover_timeout_for_job(
            &mut coordinator,
            table.physical_id,
            owner_id,
            now,
            240,
            event.map(|(_, _, event_id)| event_id),
        )
        .map_err(|error| {
            format!(
                "take over TTL job for {}.{}: {error:?}",
                table.schema, table.table
            )
        })?;
        let (job_id, expire_time) = if let Some(job_id) = takeover {
            result.resumed += 1;
            let expire_time = persisted_expire_time(&mut coordinator, &job_id)?;
            (job_id, expire_time)
        } else {
            let scan_ranges = split_ttl_scan_ranges(domain, &table)?;
            let claimed = PersistentJobStore::start_job_with_ranges(
                &mut coordinator,
                &table,
                owner_id,
                &new_job_id,
                now,
                event.is_none().then_some(schedule.job_interval_seconds),
                &scan_ranges,
            )
            .map_err(|error| {
                format!(
                    "claim TTL job for {}.{}: {error:?}",
                    table.schema, table.table
                )
            })?;
            if !claimed {
                continue;
            }
            result.claimed += 1;
            (new_job_id, table.expire_time(now))
        };
        let rows = coordinator.execute(
            "SELECT current_job_start_time FROM mysql.tidb_ttl_table_status WHERE current_job_id=%?",
            &[Datum::Text(job_id.clone())],
        ).map_err(|e| format!("read TTL job creation time: {e:?}"))?;
        let Some(Datum::Text(created)) = rows.first().and_then(|row| row.first()) else {
            return Err(format!("TTL job start time missing: {job_id}"));
        };
        let created = chrono::NaiveDateTime::parse_from_str(created, "%Y-%m-%d %H:%M:%S")
            .map_err(|e| format!("invalid TTL job start time: {e}"))?
            .and_utc()
            .timestamp() as u64;
        domain.track_ttl_job(&job_id, created);
        let scan_ranges = persisted_scan_ranges(&mut coordinator, &job_id)?;
        let scan_count = scan_ranges.len();
        if scan_count == 0 {
            return Err(format!("TTL job {job_id} has no persisted scan tasks"));
        }
        let mut total_rows = 0;
        let mut success_rows = 0;
        let mut error_rows = 0;
        for scan_range in scan_ranges {
            let statistics = Arc::new(TtlStatistics::default());
            let state = persisted_task_state(&mut coordinator, &job_id, scan_range.scan_id)?;
            statistics.restore(state.total_rows, state.success_rows, state.error_rows);
            let task = TtlScanTask {
                job_id: job_id.clone(),
                scan_id: scan_range.scan_id,
                table: table.clone(),
                expire_time,
                range_start: scan_range.start,
                range_end: scan_range.end,
                batch_size: 128,
            };
            let mut heartbeat = JobHeartbeat::start(
                Arc::clone(domain),
                table.physical_id,
                job_id.clone(),
                owner_id.into(),
                Duration::from_secs(10),
            );
            let mut scan_session =
                TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
            let mut delete_session =
                TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
            let mut checkpoint_session =
                TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
            let scan_previous =
                astersql_ttl_ttlworker::session::prepare_session_checked(&mut scan_session)
                    .map_err(|error| format!("prepare TTL scan session: {error:?}"))?;
            let delete_previous =
                astersql_ttl_ttlworker::session::prepare_session_checked(&mut delete_session)
                    .map_err(|error| format!("prepare TTL delete session: {error:?}"))?;
            let cancel_delete =
                || canceled() || heartbeat.lost() || !scheduling_enabled(now).unwrap_or(false);
            let mut limiter = ConfiguredDeleteRateLimiter {
                canceled: &cancel_delete,
            };
            let mut retry = DeleteRetryBuffer::default();
            let scan_result = task.execute_with_checkpoint(
                &mut scan_session,
                &statistics,
                state.cursor,
                |rows| {
                    if !PersistentJobStore::heartbeat(
                        &mut coordinator,
                        table.physical_id,
                        &job_id,
                        owner_id,
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    )? {
                        return Err(SessionError::Execute(
                            "TTL job ownership changed during scan".into(),
                        ));
                    }
                    let delete = DeleteTask {
                        job_id: job_id.clone(),
                        table: table.clone(),
                        rows,
                        expire_time: task.expire_time,
                        statistics: Arc::clone(&statistics),
                    };
                    let remaining = delete.do_delete(&mut delete_session, &mut limiter);
                    let needs_retry = !remaining.is_empty();
                    retry.record_task_result(delete, remaining);
                    if needs_retry {
                        return Err(SessionError::Execute(
                            "TTL delete retry required before scan checkpoint".into(),
                        ));
                    }
                    Ok(())
                },
                |cursor| {
                    checkpoint_cursor(
                        &mut checkpoint_session,
                        table.physical_id,
                        &job_id,
                        task.scan_id,
                        owner_id,
                        cursor,
                        &statistics,
                    )
                },
                cancel_delete,
            );
            while retry.len() > 0 && !canceled() && !heartbeat.lost() && scheduling_enabled(now)? {
                std::thread::park_timeout(retry.retry_interval());
                retry.retry_all(|delete| delete.do_delete(&mut delete_session, &mut limiter));
            }
            if canceled() || heartbeat.lost() || !scheduling_enabled(now)? {
                retry.drain();
            }
            heartbeat.stop();
            if heartbeat.lost() {
                return Err(format!("TTL job ownership lost during scan: {job_id}"));
            }
            if canceled() || scan_result.reason != TaskTerminateReason::Finished {
                return Err(format!(
                    "TTL scan incomplete; job remains resumable: {job_id}: {:?}",
                    scan_result.reason
                ));
            }
            astersql_ttl_ttlworker::session::restore_session_checked(
                &mut scan_session,
                scan_previous,
            )
            .map_err(|error| format!("restore TTL scan session: {error:?}"))?;
            astersql_ttl_ttlworker::session::restore_session_checked(
                &mut delete_session,
                delete_previous,
            )
            .map_err(|error| format!("restore TTL delete session: {error:?}"))?;
            let (scanned, deleted, errors) = statistics.snapshot();
            total_rows += scanned;
            success_rows += deleted;
            error_rows += errors;
        }
        let scan_task_err = String::new();
        let summary = TtlSummary {
            total_rows,
            success_rows,
            error_rows,
            scan_task_err: scan_task_err.clone(),
        };
        let summary_text = serde_json::json!({
            "total_rows": total_rows,
            "success_rows": success_rows,
            "error_rows": error_rows,
            "scan_task_err": scan_task_err,
            "total_scan_task": scan_count,
            "scheduled_scan_task": scan_count,
            "finished_scan_task": scan_count,
        })
        .to_string();
        PersistentJobStore::finish_job(
            &mut coordinator,
            table.physical_id,
            &job_id,
            owner_id,
            now,
            &summary,
            &summary_text,
        )
        .map_err(|error| format!("finish TTL job {job_id}: {error:?}"))?;
        domain.complete_ttl_job(&job_id);
        result.finished += 1;
    }
    domain.recycle_finished_ttl_jobs();
    Ok(result)
}

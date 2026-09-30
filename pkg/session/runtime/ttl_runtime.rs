// Copyright 2026 AsterSQL.

//! One scheduling pass of the SQL-backed TTL job manager. A Domain worker
//! drives this repeatedly after bootstrap; this module owns the real session,
//! scan, delete, retry, and durable completion path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_domain::Domain;
use astersql_ttl_ttlworker::del::{DeleteRateLimiter, DeleteRetryBuffer, DeleteTask};
use astersql_ttl_ttlworker::job_manager::TtlSummary;
use astersql_ttl_ttlworker::persistent::PersistentJobStore;
use astersql_ttl_ttlworker::scan::{TaskTerminateReason, TtlScanTask, TtlStatistics};
use astersql_ttl_ttlworker::session::{Datum, SessionError, WorkerSession};
use astersql_util_timeutil::time_zone::WithinDayTimePeriod;

use super::ConcreteSession;
use super::ttl_metadata::{collect_ttl_schedules, split_ttl_scan_ranges};
use super::ttl_timer::{mark_ttl_timer_fired, mark_ttl_timer_triggered, sync_ttl_timers};
use super::ttl_worker_session::TtlWorkerSqlSession;

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
    static NEXT_OWNER_ID: AtomicU64 = AtomicU64::new(1);
    let owner_id = format!(
        "{}-{}-{}",
        domain.server_id(),
        std::process::id(),
        NEXT_OWNER_ID.fetch_add(1, Ordering::Relaxed)
    );
    let weak = Arc::downgrade(domain);
    domain
        .start_ttl_job_manager(Duration::from_secs(10), move |stop| {
            let Some(domain) = weak.upgrade() else {
                return;
            };
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Err(error) = run_ttl_tick(&domain, &owner_id, now, || {
                stop.load(Ordering::Acquire) || domain.is_closed()
            }) {
                super::BgLogger().log(
                    super::LogLevel::Error,
                    "TTL job manager tick failed",
                    [super::LogField::String("error".into(), error)],
                );
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
    let schedules = collect_ttl_schedules(domain.info_schema().as_ref(), now)?;
    let mut result = TtlTickResult {
        tables: schedules.len(),
        ..TtlTickResult::default()
    };
    let mut coordinator = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
    sync_ttl_timers(&mut coordinator, &schedules, now)?;
    if !scheduling_enabled(now)? {
        return Ok(result);
    }
    for schedule in schedules {
        if canceled() || !scheduling_enabled(now)? {
            break;
        }
        let table = schedule.table;
        let new_job_id = format!(
            "ttl-{owner_id}-{}-{now}-{}",
            table.physical_id,
            NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed)
        );
        let (job_id, expire_time) = if let Some(job_id) = PersistentJobStore::takeover_timeout(
            &mut coordinator,
            table.physical_id,
            owner_id,
            now,
            240,
        )
        .map_err(|error| {
            format!(
                "take over TTL job for {}.{}: {error:?}",
                table.schema, table.table
            )
        })? {
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
                Some(schedule.job_interval_seconds),
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
        mark_ttl_timer_triggered(
            &mut coordinator,
            table.table_id,
            table.physical_id,
            &job_id,
            now,
        )?;
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
        mark_ttl_timer_fired(
            &mut coordinator,
            table.table_id,
            table.physical_id,
            now,
            &summary_text,
        )?;
        result.finished += 1;
    }
    Ok(result)
}

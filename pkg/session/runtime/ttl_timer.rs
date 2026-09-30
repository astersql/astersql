// Copyright 2026 AsterSQL.

//! Durable TTL timer metadata synchronized from one InfoSchema snapshot.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use astersql_domain::Domain;
use astersql_timer_api::{
    Context, Hook, PreSchedEventResult, TimerClient, TimerError, TimerResult, TimerShedEvent,
    WithSetSummaryData, WithSetWatermark,
};

use astersql_ttl_ttlworker::session::{Datum, SessionError, WorkerSession};
use astersql_ttl_ttlworker::timer_sync::{TIMER_HOOK_CLASS, timer_key, timer_tags};

use super::ConcreteSession;
use super::ttl_metadata::TtlSchedule;
use super::ttl_runtime::{run_ttl_event, within_ttl_window};
use super::ttl_worker_session::TtlWorkerSqlSession;

/// One timer worker owns its scan and completion watcher; Stop joins both via
/// the same thread so Domain shutdown cannot leave a detached TTL job behind.
pub(super) struct SqlTtlTimerHook {
    domain: Arc<Domain>,
    owner_id: String,
    client: Arc<dyn TimerClient>,
    stop: Arc<AtomicBool>,
    jobs: Vec<std::thread::JoinHandle<()>>,
}

impl SqlTtlTimerHook {
    pub(super) fn new(domain: Arc<Domain>, owner_id: String, client: Box<dyn TimerClient>) -> Self {
        Self {
            domain,
            owner_id,
            client: Arc::from(client),
            stop: Arc::new(AtomicBool::new(false)),
            jobs: Vec::new(),
        }
    }
}

fn timer_ids(data: &[u8]) -> TimerResult<(i64, i64)> {
    let data: serde_json::Value = serde_json::from_slice(data)
        .map_err(|error| TimerError::message(format!("invalid TTL timer data: {error}")))?;
    let table_id = data["table_id"]
        .as_i64()
        .ok_or_else(|| TimerError::message("missing TTL table ID"))?;
    let physical_id = data["physical_id"]
        .as_i64()
        .ok_or_else(|| TimerError::message("missing TTL physical ID"))?;
    Ok((table_id, physical_id))
}

pub(super) fn pre_schedule_delay(
    enabled: bool,
    table_exists: bool,
    now: u64,
    start: &str,
    end: &str,
) -> Result<Duration, String> {
    if !enabled || !table_exists || !within_ttl_window(now, start, end)? {
        Ok(Duration::from_secs(60))
    } else {
        Ok(Duration::ZERO)
    }
}

fn job_status(
    domain: &Arc<Domain>,
    table_id: i64,
    physical_id: i64,
    event_id: &str,
) -> Result<Option<(bool, serde_json::Value)>, String> {
    let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(Arc::clone(domain)));
    let rows = sql.execute(
        "SELECT summary_text,status FROM mysql.tidb_ttl_job_history WHERE table_id=%? AND parent_table_id=%? AND job_id=%?",
        &[Datum::Integer(physical_id), Datum::Integer(table_id), Datum::Text(event_id.into())],
    ).map_err(|error| format!("trace TTL job {event_id}: {error:?}"))?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let finished = matches!(row.get(1), Some(Datum::Text(status)) if matches!(status.as_str(), "finished" | "timeout" | "cancelled"));
    let summary = match row.first() {
        Some(Datum::Text(text)) if text != "<nil>" && !text.is_empty() => {
            serde_json::from_str(text)
                .map_err(|error| format!("invalid TTL job summary: {error}"))?
        }
        _ => serde_json::Value::Null,
    };
    Ok(Some((finished, summary)))
}

impl Hook for SqlTtlTimerHook {
    fn Start(&mut self) {}

    fn Stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for job in self.jobs.drain(..) {
            job.thread().unpark();
            let _ = job.join();
        }
    }

    fn OnPreSchedEvent(
        &mut self,
        _ctx: &Context,
        event: &dyn TimerShedEvent,
    ) -> TimerResult<PreSchedEventResult> {
        let mut result = PreSchedEventResult::default();
        let now = chrono::Utc::now().timestamp() as u64;
        let timer = event
            .Timer()
            .ok_or_else(|| TimerError::message("TTL timer missing"))?;
        let (table_id, physical_id) = timer_ids(&timer.Data)?;
        let table_exists =
            super::ttl_metadata::collect_ttl_schedules(self.domain.info_schema().as_ref(), now)
                .map_err(TimerError::message)?
                .iter()
                .any(|schedule| {
                    schedule.table.table_id == table_id && schedule.table.physical_id == physical_id
                });
        result.Delay = pre_schedule_delay(
            astersql_sessionctx_vardef::EnableTTLJob.Load(),
            table_exists,
            now,
            &astersql_sessionctx_vardef::TTLJobScheduleWindowStartTime.Load(),
            &astersql_sessionctx_vardef::TTLJobScheduleWindowEndTime.Load(),
        )
        .map_err(TimerError::message)?;
        Ok(result)
    }

    fn OnSchedEvent(&mut self, _ctx: &Context, event: &dyn TimerShedEvent) -> TimerResult<()> {
        let mut active = Vec::new();
        for job in self.jobs.drain(..) {
            if job.is_finished() {
                let _ = job.join();
            } else {
                active.push(job);
            }
        }
        self.jobs = active;
        let timer = event
            .Timer()
            .ok_or_else(|| TimerError::message("TTL timer missing"))?;
        let event_id = event.EventID();
        let (table_id, physical_id) = timer_ids(&timer.Data)?;
        let finished_job = matches!(
            job_status(&self.domain, table_id, physical_id, &event_id)
                .map_err(TimerError::message)?,
            Some((true, _))
        );
        let event_start = timer
            .EventStart
            .ok_or_else(|| TimerError::message("TTL event start missing"))?;
        let domain = Arc::clone(&self.domain);
        let owner_id = self.owner_id.clone();
        let client = Arc::clone(&self.client);
        let stop = Arc::clone(&self.stop);
        let enabled = timer.Enable;
        let prior_watermark = timer.Watermark.unwrap_or(event_start);
        let timer_id = timer.ID;
        self.jobs.push(
            std::thread::Builder::new()
                .name("ttl-timer-job".into())
                .spawn(move || {
                    if !finished_job {
                        let now = chrono::Utc::now().timestamp() as u64;
                        let valid_table = super::ttl_metadata::collect_ttl_schedules(
                            domain.info_schema().as_ref(),
                            now,
                        )
                        .is_ok_and(|schedules| {
                            schedules.iter().any(|schedule| {
                                schedule.table.table_id == table_id
                                    && schedule.table.physical_id == physical_id
                            })
                        });
                        if !enabled
                            || !valid_table
                            || now.saturating_sub(event_start.timestamp() as u64) > 600
                        {
                            let _ = client.CloseTimerEvent(
                                &Context::background(),
                                &timer_id,
                                &event_id,
                                vec![WithSetWatermark(prior_watermark)],
                            );
                            return;
                        }
                        if let Err(error) = run_ttl_event(
                            &domain,
                            &owner_id,
                            now,
                            table_id,
                            physical_id,
                            &event_id,
                            || stop.load(Ordering::Acquire) || domain.is_closed(),
                        ) {
                            super::BgLogger().log(
                                super::LogLevel::Error,
                                "TTL timer event job failed",
                                [super::LogField::String("error".into(), error)],
                            );
                        }
                    }
                    let wait_started = std::time::Instant::now();
                    while !stop.load(Ordering::Acquire) && !domain.is_closed() {
                        let ctx = Context::background();
                        let Ok(current) = client.GetTimerByID(&ctx, &timer_id) else {
                            return;
                        };
                        if current.EventID != event_id {
                            return;
                        }
                        if let Ok(Some((true, summary))) =
                            job_status(&domain, table_id, physical_id, &event_id)
                        {
                            let data = serde_json::json!({
                                "last_job_request_id": event_id,
                                "last_job_summary": summary,
                            })
                            .to_string()
                            .into_bytes();
                            if client
                                .CloseTimerEvent(
                                    &ctx,
                                    &timer_id,
                                    &event_id,
                                    vec![WithSetWatermark(event_start), WithSetSummaryData(data)],
                                )
                                .is_ok()
                            {
                                return;
                            }
                        }
                        if wait_started.elapsed() > Duration::from_secs(600)
                            && matches!(
                                job_status(&domain, table_id, physical_id, &event_id),
                                Ok(None)
                            )
                        {
                            let _ = client.CloseTimerEvent(
                                &ctx,
                                &timer_id,
                                &event_id,
                                vec![WithSetWatermark(prior_watermark)],
                            );
                            return;
                        }
                        std::thread::park_timeout(Duration::from_secs(10));
                    }
                })
                .map_err(|error| TimerError::message(format!("start TTL timer job: {error}")))?,
        );
        Ok(())
    }
}

pub(super) fn sync_ttl_timers(
    session: &mut TtlWorkerSqlSession,
    schedules: &[TtlSchedule],
    now: u64,
) -> Result<(), String> {
    let mut live = HashSet::with_capacity(schedules.len());
    for schedule in schedules {
        let table = &schedule.table;
        let key = timer_key(table.table_id, table.physical_id);
        live.insert(key.clone());
        let tags = timer_tags(table);
        let data = serde_json::json!({
            "table_id": table.table_id,
            "physical_id": table.physical_id,
        })
        .to_string();
        let ext = serde_json::json!({"tags": tags, "manual": {}, "event": {}}).to_string();
        let existing = session
            .execute(
                "SELECT SCHED_POLICY_EXPR,ENABLE,TIMER_EXT FROM mysql.tidb_timers WHERE NAMESPACE='default' AND TIMER_KEY=%?",
                &[Datum::Text(key.clone())],
            )
            .map_err(|error| format!("read TTL timer {key}: {error:?}"))?;
        if let Some(row) = existing.first() {
            let expression = match row.first() {
                Some(Datum::Text(value)) => value.as_str(),
                _ => "",
            };
            let enabled = matches!(row.get(1), Some(Datum::Text(value)) if value == "1");
            let mut old_ext = match row.get(2) {
                Some(Datum::Text(value)) => serde_json::from_str::<serde_json::Value>(value)
                    .map_err(|error| format!("invalid TTL timer extension for {key}: {error}"))?,
                _ => serde_json::json!({}),
            };
            let old_tags = old_ext.get("tags").cloned();
            if expression != schedule.job_interval_expression
                || !enabled
                || old_tags != Some(serde_json::json!(tags))
            {
                old_ext["tags"] = serde_json::json!(tags);
                session
                    .execute(
                        "UPDATE mysql.tidb_timers SET TIMER_DATA=%?,SCHED_POLICY_EXPR=%?,TIMER_EXT=%?,ENABLE=1,VERSION=VERSION+1 WHERE NAMESPACE='default' AND TIMER_KEY=%?",
                        &[
                            Datum::Bytes(data.into_bytes()),
                            Datum::Text(schedule.job_interval_expression.clone()),
                            Datum::Text(old_ext.to_string()),
                            Datum::Text(key.clone()),
                        ],
                    )
                    .map_err(|error| format!("update TTL timer {key}: {error:?}"))?;
            }
            continue;
        }
        let watermark = session
            .execute(
                "SELECT last_job_start_time FROM mysql.tidb_ttl_table_status WHERE table_id=%?",
                &[Datum::Integer(table.physical_id)],
            )
            .map_err(|error| format!("read TTL timer watermark {key}: {error:?}"))?
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .filter(|value| !matches!(value, Datum::Text(text) if text == "<nil>"));
        session
            .execute(
                "INSERT INTO mysql.tidb_timers (NAMESPACE,TIMER_KEY,TIMER_DATA,TIMEZONE,SCHED_POLICY_TYPE,SCHED_POLICY_EXPR,HOOK_CLASS,WATERMARK,ENABLE,TIMER_EXT,EVENT_STATUS,EVENT_ID,VERSION) VALUES ('default',%? ,%?,'UTC','INTERVAL',%? ,%? ,%? ,1,%? ,'IDLE','',1)",
                &[
                    Datum::Text(key.clone()),
                    Datum::Bytes(data.into_bytes()),
                    Datum::Text(schedule.job_interval_expression.clone()),
                    Datum::Text(TIMER_HOOK_CLASS.into()),
                    // Go's zero watermark schedules a new TTL timer immediately.
                    // The generic Rust timer treats NULL as no next event.
                    watermark.unwrap_or_else(|| Datum::Text("1970-01-01 00:00:01".into())),
                    Datum::Text(ext),
                ],
            )
            .map_err(|error| format!("create TTL timer {key}: {error:?}"))?;
    }

    let timers = session
        .execute(
            "SELECT TIMER_KEY,ENABLE,CREATE_TIME FROM mysql.tidb_timers WHERE NAMESPACE='default' AND HOOK_CLASS='tidb.ttl'",
            &[],
        )
        .map_err(|error| format!("list TTL timers: {error:?}"))?;
    for timer in timers {
        let Some(Datum::Text(key)) = timer.first() else {
            continue;
        };
        if live.contains(key) {
            continue;
        }
        let created = timer
            .get(2)
            .and_then(|value| match value {
                Datum::Text(value) => Some(value),
                _ => None,
            })
            .and_then(|value| {
                chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()
            })
            .and_then(|time| u64::try_from(time.and_utc().timestamp()).ok());
        let expired = created.is_some_and(|created| now.saturating_sub(created) > 600);
        let (sql, args) = if expired {
            (
                "DELETE FROM mysql.tidb_timers WHERE NAMESPACE='default' AND TIMER_KEY=%?",
                vec![Datum::Text(key.clone())],
            )
        } else if matches!(timer.get(1), Some(Datum::Text(value)) if value == "1") {
            (
                "UPDATE mysql.tidb_timers SET ENABLE=0,VERSION=VERSION+1 WHERE NAMESPACE='default' AND TIMER_KEY=%?",
                vec![Datum::Text(key.clone())],
            )
        } else {
            continue;
        };
        session
            .execute(sql, &args)
            .map_err(|error: SessionError| format!("retire TTL timer {key}: {error:?}"))?;
    }
    Ok(())
}

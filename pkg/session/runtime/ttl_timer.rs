// Copyright 2026 AsterSQL.

//! Durable TTL timer metadata synchronized from one InfoSchema snapshot.

use std::collections::HashSet;

use astersql_ttl_ttlworker::session::{Datum, SessionError, WorkerSession};
use astersql_ttl_ttlworker::timer_sync::{TIMER_HOOK_CLASS, timer_key, timer_tags};

use super::ttl_metadata::TtlSchedule;
use super::ttl_worker_session::TtlWorkerSqlSession;

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
                    watermark.unwrap_or(Datum::Null),
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

pub(super) fn mark_ttl_timer_triggered(
    session: &mut TtlWorkerSqlSession,
    table_id: i64,
    physical_id: i64,
    job_id: &str,
    now: u64,
) -> Result<(), String> {
    let key = timer_key(table_id, physical_id);
    session
        .execute(
            "UPDATE mysql.tidb_timers SET EVENT_STATUS='TRIGGER',EVENT_ID=%?,EVENT_START=FROM_UNIXTIME(%?),EVENT_DATA=%?,VERSION=VERSION+1 WHERE NAMESPACE='default' AND TIMER_KEY=%?",
            &[
                Datum::Text(job_id.into()),
                Datum::Unsigned(now),
                Datum::Bytes(job_id.as_bytes().to_vec()),
                Datum::Text(key.clone()),
            ],
        )
        .map_err(|error| format!("trigger TTL timer {key}: {error:?}"))?;
    Ok(())
}

pub(super) fn mark_ttl_timer_fired(
    session: &mut TtlWorkerSqlSession,
    table_id: i64,
    physical_id: i64,
    start_time: u64,
    summary: &str,
) -> Result<(), String> {
    let key = timer_key(table_id, physical_id);
    session
        .execute(
            "UPDATE mysql.tidb_timers SET WATERMARK=FROM_UNIXTIME(%?),EVENT_STATUS='IDLE',EVENT_ID='',EVENT_DATA=NULL,EVENT_START=NULL,SUMMARY_DATA=%?,VERSION=VERSION+1 WHERE NAMESPACE='default' AND TIMER_KEY=%?",
            &[
                Datum::Unsigned(start_time),
                Datum::Bytes(summary.as_bytes().to_vec()),
                Datum::Text(key.clone()),
            ],
        )
        .map_err(|error| format!("advance TTL timer {key}: {error:?}"))?;
    Ok(())
}

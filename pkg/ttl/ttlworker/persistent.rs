// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Transactional SQL state for TTL jobs. The system tables are the authority
//! across nodes; in-memory job maps are only worker-side caches.

use crate::session::{Datum, PhysicalTable, SessionError, WorkerSession};

/// A table-level TTL job lock backed by the three mysql TTL system tables.
pub struct PersistentJobStore;

impl PersistentJobStore {
    /// Take over a job whose owner heartbeat has expired. The status row is
    /// locked before changing ownership so competing managers cannot both win.
    pub fn takeover_timeout(
        session: &mut dyn WorkerSession,
        table_id: i64,
        new_owner_id: &str,
        now: u64,
        timeout_seconds: u64,
    ) -> Result<Option<String>, SessionError> {
        Self::takeover_timeout_for_job(session, table_id, new_owner_id, now, timeout_seconds, None)
    }

    /// Take over only the job belonging to the current timer event.
    pub fn takeover_timeout_for_job(
        session: &mut dyn WorkerSession,
        table_id: i64,
        new_owner_id: &str,
        now: u64,
        timeout_seconds: u64,
        expected_job_id: Option<&str>,
    ) -> Result<Option<String>, SessionError> {
        session.execute("BEGIN PESSIMISTIC", &[])?;
        let result = (|| {
            let mut args = vec![
                Datum::Integer(table_id),
                Datum::Unsigned(now.saturating_sub(timeout_seconds)),
            ];
            let sql = if let Some(expected_job_id) = expected_job_id {
                args.push(Datum::Text(expected_job_id.into()));
                "SELECT current_job_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_id IS NOT NULL AND current_job_owner_hb_time < FROM_UNIXTIME(%?) AND current_job_id=%? FOR UPDATE NOWAIT"
            } else {
                "SELECT current_job_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_id IS NOT NULL AND current_job_owner_hb_time < FROM_UNIXTIME(%?) FOR UPDATE NOWAIT"
            };
            let rows = session.execute(sql, &args)?;
            let Some(Datum::Text(job_id)) = rows.first().and_then(|row| row.first()) else {
                return Ok(None);
            };
            session.execute(
                "UPDATE mysql.tidb_ttl_table_status SET current_job_owner_id=%?,current_job_owner_hb_time=FROM_UNIXTIME(%?) WHERE table_id=%? AND current_job_id=%?",
                &[
                    Datum::Text(new_owner_id.into()),
                    Datum::Unsigned(now),
                    Datum::Integer(table_id),
                    Datum::Text(job_id.clone()),
                ],
            )?;
            Ok::<_, SessionError>(Some(job_id.clone()))
        })();
        match result {
            Ok(job_id) => {
                if let Err(error) = session.execute("COMMIT", &[]) {
                    session.avoid_reuse();
                    return Err(error);
                }
                Ok(job_id)
            }
            Err(error) => {
                if session.execute("ROLLBACK", &[]).is_err() {
                    session.avoid_reuse();
                }
                Err(error)
            }
        }
    }

    /// Refresh a claimed job's lease only while this owner and job still
    /// match the durable status row.
    pub fn heartbeat(
        session: &mut dyn WorkerSession,
        table_id: i64,
        job_id: &str,
        owner_id: &str,
        now: u64,
    ) -> Result<bool, SessionError> {
        session.execute(
            "UPDATE mysql.tidb_ttl_table_status SET current_job_owner_hb_time=FROM_UNIXTIME(%?) WHERE table_id=%? AND current_job_id=%? AND current_job_owner_id=%?",
            &[
                Datum::Unsigned(now),
                Datum::Integer(table_id),
                Datum::Text(job_id.into()),
                Datum::Text(owner_id.into()),
            ],
        )?;
        if session
            .execute("SELECT ROW_COUNT()", &[])?
            .first()
            .and_then(|row| row.first())
            == Some(&Datum::Text("1".into()))
        {
            return Ok(true);
        }
        // MySQL reports zero changed rows when the heartbeat timestamp was
        // already equal to `now`, while this owner still holds the job.
        Ok(!session
            .execute(
                "SELECT table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_id=%? AND current_job_owner_id=%?",
                &[
                    Datum::Integer(table_id),
                    Datum::Text(job_id.into()),
                    Datum::Text(owner_id.into()),
                ],
            )?
            .is_empty())
    }

    /// Move a completed job into last-job status, delete its scan tasks, and
    /// finish its history in one pessimistic transaction, matching Go's
    /// `ttlJob.finish` ordering.
    pub fn finish_job(
        session: &mut dyn WorkerSession,
        table_id: i64,
        job_id: &str,
        owner_id: &str,
        now: u64,
        summary: &crate::job_manager::TtlSummary,
        summary_text: &str,
    ) -> Result<(), SessionError> {
        session.execute("BEGIN PESSIMISTIC", &[])?;
        let result = (|| {
            session.execute(
                "UPDATE mysql.tidb_ttl_table_status SET last_job_id=current_job_id,last_job_start_time=current_job_start_time,last_job_finish_time=FROM_UNIXTIME(%?),last_job_ttl_expire=current_job_ttl_expire,last_job_summary=%?,current_job_id=NULL,current_job_owner_id=NULL,current_job_owner_hb_time=NULL,current_job_start_time=NULL,current_job_ttl_expire=NULL,current_job_state=NULL,current_job_status=NULL,current_job_status_update_time=NULL WHERE table_id=%? AND current_job_id=%? AND current_job_owner_id=%?",
                &[
                    Datum::Unsigned(now),
                    Datum::Text(summary_text.into()),
                    Datum::Integer(table_id),
                    Datum::Text(job_id.into()),
                    Datum::Text(owner_id.into()),
                ],
            )?;
            if session
                .execute("SELECT ROW_COUNT()", &[])?
                .first()
                .and_then(|row| row.first())
                != Some(&Datum::Text("1".into()))
            {
                return Err(SessionError::Execute(
                    "TTL job ownership changed before completion".into(),
                ));
            }
            session.execute(
                crate::job::REMOVE_TASK_FOR_JOB_SQL,
                &[Datum::Text(job_id.into())],
            )?;
            session.execute(
                "UPDATE mysql.tidb_ttl_job_history SET finish_time=FROM_UNIXTIME(%?),summary_text=%?,expired_rows=%?,deleted_rows=%?,error_delete_rows=%?,status='finished' WHERE job_id=%?",
                &[
                    Datum::Unsigned(now),
                    Datum::Text(summary_text.into()),
                    Datum::Unsigned(summary.total_rows),
                    Datum::Unsigned(summary.success_rows),
                    Datum::Unsigned(summary.error_rows),
                    Datum::Text(job_id.into()),
                ],
            )?;
            Ok::<(), SessionError>(())
        })();
        match result {
            Ok(()) => {
                if let Err(error) = session.execute("COMMIT", &[]) {
                    session.avoid_reuse();
                    return Err(error);
                }
                Ok(())
            }
            Err(error) => {
                if session.execute("ROLLBACK", &[]).is_err() {
                    session.avoid_reuse();
                }
                Err(error)
            }
        }
    }

    /// Atomically claim one table, create its history, and enqueue a full-range
    /// scan. An existing current job or an unmet schedule interval returns
    /// `false` without changing any system table.
    pub fn start_job(
        session: &mut dyn WorkerSession,
        table: &PhysicalTable,
        owner_id: &str,
        job_id: &str,
        now: u64,
        schedule_interval_seconds: Option<u64>,
    ) -> Result<bool, SessionError> {
        Self::start_job_with_ranges(
            session,
            table,
            owner_id,
            job_id,
            now,
            schedule_interval_seconds,
            &[astersql_ttl_cache::table::newFullRange()],
            None,
        )
    }

    /// Claim a table and persist every Region-derived scan range in the same
    /// transaction as the job lock and history record.
    pub fn start_job_with_ranges(
        session: &mut dyn WorkerSession,
        table: &PhysicalTable,
        owner_id: &str,
        job_id: &str,
        now: u64,
        schedule_interval_seconds: Option<u64>,
        ranges: &[astersql_ttl_cache::table::ScanRange],
        scan_index_id: Option<i64>,
    ) -> Result<bool, SessionError> {
        if !table.ttl_enabled {
            return Ok(false);
        }
        session.execute("BEGIN PESSIMISTIC", &[])?;
        let result = Self::start_job_in_transaction(
            session,
            table,
            owner_id,
            job_id,
            now,
            schedule_interval_seconds,
            ranges,
            scan_index_id,
        );
        match result {
            Ok(true) => {
                if let Err(error) = session.execute("COMMIT", &[]) {
                    session.avoid_reuse();
                    return Err(error);
                }
                Ok(true)
            }
            Ok(false) => {
                if let Err(error) = session.execute("ROLLBACK", &[]) {
                    session.avoid_reuse();
                    return Err(error);
                }
                Ok(false)
            }
            Err(error) => {
                if session.execute("ROLLBACK", &[]).is_err() {
                    session.avoid_reuse();
                }
                Err(error)
            }
        }
    }

    fn start_job_in_transaction(
        session: &mut dyn WorkerSession,
        table: &PhysicalTable,
        owner_id: &str,
        job_id: &str,
        now: u64,
        schedule_interval_seconds: Option<u64>,
        ranges: &[astersql_ttl_cache::table::ScanRange],
        scan_index_id: Option<i64>,
    ) -> Result<bool, SessionError> {
        let table_id = Datum::Integer(table.physical_id);
        let lock_sql =
            "SELECT table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? FOR UPDATE NOWAIT";
        if session
            .execute(lock_sql, std::slice::from_ref(&table_id))?
            .is_empty()
        {
            session.execute(
                crate::job_manager::INSERT_NEW_TABLE_INTO_STATUS_SQL,
                &[table_id.clone(), Datum::Integer(table.table_id)],
            )?;
            if session
                .execute(lock_sql, std::slice::from_ref(&table_id))?
                .is_empty()
            {
                return Err(SessionError::Execute(
                    "TTL status row missing after insert".into(),
                ));
            }
        }
        if !session
            .execute(
                "SELECT current_job_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND current_job_id IS NOT NULL",
                std::slice::from_ref(&table_id),
            )?
            .is_empty()
        {
            return Ok(false);
        }
        if let Some(interval) = schedule_interval_seconds {
            let earliest = now.saturating_sub(interval);
            if !session
                .execute(
                    "SELECT table_id FROM mysql.tidb_ttl_table_status WHERE table_id=%? AND last_job_start_time > FROM_UNIXTIME(%?)",
                    &[table_id.clone(), Datum::Unsigned(earliest)],
                )?
                .is_empty()
            {
                return Ok(false);
            }
        }
        let expire = table.expire_time(now);
        session.execute(
            "UPDATE mysql.tidb_ttl_table_status SET current_job_id=%?,current_job_owner_id=%?,current_job_owner_hb_time=FROM_UNIXTIME(%?),current_job_start_time=FROM_UNIXTIME(%?),current_job_ttl_expire=FROM_UNIXTIME(%?),current_job_status='running',current_job_status_update_time=FROM_UNIXTIME(%?) WHERE table_id=%? AND current_job_id IS NULL",
            &[
                Datum::Text(job_id.into()),
                Datum::Text(owner_id.into()),
                Datum::Unsigned(now),
                Datum::Unsigned(now),
                Datum::Unsigned(expire),
                Datum::Unsigned(now),
                table_id.clone(),
            ],
        )?;
        let affected = session.execute("SELECT ROW_COUNT()", &[])?;
        if affected.first().and_then(|row| row.first()) != Some(&Datum::Text("1".into())) {
            return Err(SessionError::Execute(
                "TTL status lock affected no row".into(),
            ));
        }
        session.execute(
            "INSERT INTO mysql.tidb_ttl_job_history (job_id,table_id,parent_table_id,table_schema,table_name,partition_name,create_time,finish_time,ttl_expire,status) VALUES (%?,%?,%?,%?,%?,%?,FROM_UNIXTIME(%?),FROM_UNIXTIME(%?),FROM_UNIXTIME(%?),'running')",
            &[
                Datum::Text(job_id.into()),
                table_id.clone(),
                Datum::Integer(table.table_id),
                Datum::Text(table.schema.clone()),
                Datum::Text(table.table.clone()),
                table.partition_name.as_ref().map_or(Datum::Null, |name| Datum::Text(name.clone())),
                Datum::Unsigned(now),
                Datum::Unsigned(1),
                Datum::Unsigned(expire),
            ],
        )?;
        for (scan_id, range) in ranges.iter().enumerate() {
            let start = astersql_ttl_cache::task::EncodeDatums(&range.Start)
                .map_err(SessionError::Execute)?;
            let end = astersql_ttl_cache::task::EncodeDatums(&range.End)
                .map_err(SessionError::Execute)?;
            session.execute(
                "INSERT INTO mysql.tidb_ttl_task (job_id,table_id,scan_id,scan_range_start,scan_range_end,expire_time,created_time,scan_index_id) VALUES (%?,%?,%?,%?,%?,FROM_UNIXTIME(%?),FROM_UNIXTIME(%?),%?)",
                &[
                    Datum::Text(job_id.into()),
                    table_id.clone(),
                    Datum::Integer(scan_id as i64),
                    Datum::Bytes(start),
                    Datum::Bytes(end),
                    Datum::Unsigned(expire),
                    Datum::Unsigned(now),
                    scan_index_id.map(Datum::Integer).unwrap_or(Datum::Null),
                ],
            )?;
        }
        Ok(true)
    }
}

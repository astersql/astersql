// Copyright 2026 AsterSQL.

//! MVCC-fenced deletion primitives for materialized view logs.

use super::*;

const DEFAULT_MLOG_PURGE_BATCH_SIZE: usize = 10_000;
const MLOG_PURGE_ADAPTIVE_MAX_BUDGET: std::time::Duration = std::time::Duration::from_secs(590);
const MLOG_PURGE_ADAPTIVE_BATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(200);
const MLOG_PURGE_ADAPTIVE_MIN_BATCH_SIZE: usize = 8_000;

struct MLogPurgeThrottlePlan {
    target_rate: f64,
    pending_rows: usize,
    min_rate: f64,
    deadline: std::time::Instant,
    no_wait_streak: usize,
}

#[derive(Default)]
struct MLogPendingRowStats {
    pending_rows: usize,
    row_id_bounds: Option<(i64, i64)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MLogRowIDRange {
    start: i64,
    end: i64,
}

fn mlog_purge_row_id_ranges(stats: &MLogPendingRowStats, shard_bits: u64) -> Vec<MLogRowIDRange> {
    let Some((min, max)) = stats.row_id_bounds else {
        return Vec::new();
    };
    if stats.pending_rows == 0 || min > max {
        return Vec::new();
    }
    let mut count = (stats.pending_rows / MLOG_PURGE_ADAPTIVE_MIN_BATCH_SIZE).clamp(1, 16);
    if shard_bits > 0 {
        let Some(bucket_count) = 1_usize.checked_shl(shard_bits as u32) else {
            return vec![MLogRowIDRange {
                start: min,
                end: max,
            }];
        };
        count = count.min(bucket_count);
        count = 1_usize << (usize::BITS - 1 - count.leading_zeros());
        if count <= 1 || min >= max {
            return vec![MLogRowIDRange {
                start: min,
                end: max,
            }];
        }
        let buckets_per_range = bucket_count / count;
        let incremental_bits = 63_u64.saturating_sub(shard_bits);
        let mut ranges = Vec::with_capacity(count);
        for index in 0..count {
            let start = ((index * buckets_per_range) as u64)
                .checked_shl(incremental_bits as u32)
                .unwrap_or(0) as i64;
            let end = if index + 1 == count {
                max
            } else {
                (((index + 1) * buckets_per_range) as u64)
                    .checked_shl(incremental_bits as u32)
                    .unwrap_or(0)
                    .wrapping_sub(1) as i64
            };
            let start = start.max(min);
            let end = end.min(max);
            if start <= end {
                ranges.push(MLogRowIDRange { start, end });
            }
        }
        return ranges;
    }
    if count <= 1 || min >= max {
        return vec![MLogRowIDRange {
            start: min,
            end: max,
        }];
    }
    let span = i128::from(max) - i128::from(min) + 1;
    if span <= count as i128 {
        return vec![MLogRowIDRange {
            start: min,
            end: max,
        }];
    }
    let step = span / count as i128;
    (0..count)
        .map(|index| {
            let start = (i128::from(min) + index as i128 * step) as i64;
            let end = if index + 1 == count {
                max
            } else {
                (i128::from(min) + (index as i128 + 1) * step - 1) as i64
            };
            MLogRowIDRange { start, end }
        })
        .collect()
}

impl MLogPurgeThrottlePlan {
    fn new(
        pending_rows: usize,
        deadline: std::time::Instant,
        min_rate: f64,
        ratio: f64,
    ) -> Option<Self> {
        let budget = deadline.checked_duration_since(std::time::Instant::now())?;
        if pending_rows == 0 || budget.is_zero() || ratio <= 0.0 || min_rate <= 0.0 {
            return None;
        }
        let target_rate = (pending_rows as f64 / (budget.as_secs_f64() * ratio)).max(min_rate);
        Some(Self {
            target_rate,
            pending_rows,
            min_rate,
            deadline,
            no_wait_streak: 0,
        })
    }

    fn batch_size(&self, configured: usize) -> usize {
        let adaptive = (self.target_rate * MLOG_PURGE_ADAPTIVE_BATCH_WINDOW.as_secs_f64())
            .ceil()
            .max(MLOG_PURGE_ADAPTIVE_MIN_BATCH_SIZE as f64) as usize;
        configured.min(adaptive).max(1)
    }

    fn sleep_duration(
        &mut self,
        started: std::time::Instant,
        deleted: usize,
    ) -> std::time::Duration {
        let expected = std::time::Duration::from_secs_f64(deleted as f64 / self.target_rate);
        let sleep = expected.saturating_sub(started.elapsed());
        if sleep.is_zero() {
            self.no_wait_streak += 1;
            if self.no_wait_streak >= 2 {
                let remaining = self.pending_rows.saturating_sub(deleted);
                if let Some(budget) = self
                    .deadline
                    .checked_duration_since(std::time::Instant::now())
                    && remaining > 0
                    && !budget.is_zero()
                {
                    self.target_rate = (remaining as f64 / budget.as_secs_f64()).max(self.min_rate);
                }
            }
        } else {
            self.no_wait_streak = 0;
        }
        sleep
    }
}

fn pending_mlog_rows(
    snapshot: &dyn kv::Snapshot,
    log_id: i64,
    last_purged_tso: Option<u64>,
    safe_purge_tso: u64,
) -> SessionResult<MLogPendingRowStats> {
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(log_id).0);
    let mut iterator = snapshot
        .Iter(prefix.clone(), Some(prefix.PrefixNext()))
        .map_err(|error| session_error("count pending MLog rows", error))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let result = (|| {
        let mut stats = MLogPendingRowStats::default();
        let mut all_integer_handles = true;
        while iterator.Valid() {
            if std::time::Instant::now() >= deadline {
                return Err(SessionError::new("MLog pending row count timed out"));
            }
            let commit_ts = snapshot
                .Get(&kv::Context::default(), iterator.Key(), &[])
                .map_err(|error| session_error("read pending MLog commit ts", error))?
                .CommitTs;
            if commit_ts == 0 {
                return Err(SessionError::new(
                    "MLog record commit timestamp is unavailable",
                ));
            }
            if commit_ts <= safe_purge_tso && last_purged_tso.is_none_or(|last| commit_ts > last) {
                stats.pending_rows += 1;
                let handle = astersql_tablecodec::DecodeRowKey(iterator.Key())
                    .map_err(|error| session_error("decode pending MLog row ID", error))?;
                if handle.IsInt() {
                    let row_id = handle.IntValue();
                    stats.row_id_bounds =
                        Some(stats.row_id_bounds.map_or((row_id, row_id), |(min, max)| {
                            (min.min(row_id), max.max(row_id))
                        }));
                } else {
                    all_integer_handles = false;
                }
            }
            iterator
                .Next()
                .map_err(|error| session_error("advance pending MLog scan", error))?;
        }
        if !all_integer_handles {
            stats.row_id_bounds = None;
        }
        Ok(stats)
    })();
    iterator.Close();
    result
}

pub(crate) fn purge_mlog_snapshot_batch(
    transaction: &mut dyn kv::Transaction,
    log_id: i64,
    last_purged_tso: Option<u64>,
    safe_purge_tso: u64,
    batch_size: usize,
) -> SessionResult<usize> {
    purge_mlog_snapshot_batch_from(
        transaction,
        log_id,
        last_purged_tso,
        safe_purge_tso,
        batch_size,
        None,
        None,
    )
    .map(|(deleted, _, _)| deleted)
}

fn purge_mlog_snapshot_batch_from(
    transaction: &mut dyn kv::Transaction,
    log_id: i64,
    last_purged_tso: Option<u64>,
    safe_purge_tso: u64,
    batch_size: usize,
    after: Option<kv::Key>,
    row_id_range: Option<MLogRowIDRange>,
) -> SessionResult<(usize, Option<kv::Key>, bool)> {
    if batch_size == 0 {
        return Err(SessionError::new("MLog purge batch size must be positive"));
    }
    if safe_purge_tso == 0 {
        return Ok((0, None, true));
    }
    let prefix = kv::Key(astersql_tablecodec::GenTableRecordPrefix(log_id).0);
    let range_start = row_id_range.map(|range| {
        kv::Key(
            astersql_tablecodec::EncodeRowKeyWithHandle(
                log_id,
                Box::new(astersql_tablecodec::kv::IntHandle(range.start)),
            )
            .0,
        )
    });
    let range_end = row_id_range.map(|range| {
        kv::Key(
            astersql_tablecodec::EncodeRowKeyWithHandle(
                log_id,
                Box::new(astersql_tablecodec::kv::IntHandle(range.end)),
            )
            .0,
        )
        .Next()
    });
    let mut start = after.map_or(prefix.clone(), |key| key.Next());
    if let Some(range_start) = range_start
        && start.0 < range_start.0
    {
        start = range_start;
    }
    let end = range_end.unwrap_or_else(|| prefix.PrefixNext());
    let snapshot = transaction.GetSnapshot();
    let mut iterator = snapshot
        .Iter(start, Some(end))
        .map_err(|error| session_error("scan MLog purge keys", error))?;
    let result = (|| {
        let mut keys = Vec::with_capacity(batch_size.min(1024));
        let mut last_seen = None;
        while iterator.Valid() && keys.len() < batch_size {
            let key = iterator.Key();
            let commit_ts = snapshot
                .Get(&kv::Context::default(), key.clone(), &[])
                .map_err(|error| session_error("read MLog purge commit ts", error))?
                .CommitTs;
            if commit_ts == 0 {
                return Err(SessionError::new(
                    "MLog record commit timestamp is unavailable",
                ));
            }
            if commit_ts <= safe_purge_tso && last_purged_tso.is_none_or(|last| commit_ts > last) {
                keys.push(key.clone());
            }
            last_seen = Some(key);
            iterator
                .Next()
                .map_err(|error| session_error("advance MLog purge keys", error))?;
        }
        Ok((keys, last_seen, !iterator.Valid()))
    })();
    iterator.Close();
    let (keys, last_seen, exhausted) = result?;
    let deleted = keys.len();
    for key in keys {
        transaction
            .Delete(key)
            .map_err(|error| session_error("delete MLog record", error))?;
    }
    Ok((deleted, last_seen, exhausted))
}

fn purge_sql_row(session: &ConcreteSession, sql: &str) -> SessionResult<Option<Vec<String>>> {
    let mut sets = session.execute(sql)?;
    let Some(mut set) = sets.pop() else {
        return Err(SessionError::new("MLog purge query returned no result set"));
    };
    let row = set.next_row()?;
    set.close()?;
    Ok(row)
}

fn purge_sql_rows(session: &ConcreteSession, sql: &str) -> SessionResult<Vec<Vec<String>>> {
    let mut sets = session.execute(sql)?;
    let Some(mut set) = sets.pop() else {
        return Err(SessionError::new("MLog purge query returned no result set"));
    };
    let mut rows = Vec::new();
    while let Some(row) = set.next_row()? {
        rows.push(row);
    }
    set.close()?;
    Ok(rows)
}

fn purge_sql_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}

fn purge_history_time() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string()
}

fn purge_history_duration(started: std::time::Instant) -> String {
    let micros = started.elapsed().as_micros();
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn resolve_mlog_database(explicit: &str, current: &str) -> SessionResult<String> {
    let database = if explicit.is_empty() {
        current.to_owned()
    } else {
        explicit.to_owned()
    };
    if database.is_empty() {
        return Err(SessionError::new("No database selected"));
    }
    Ok(database)
}

fn finalize_purge_history(session: &ConcreteSession, sql: &str) -> SessionResult<()> {
    if let Err(first) = session.execute(sql) {
        session.execute(sql).map_err(|second| {
            SessionError::new(format!(
                "finalize MLog purge history failed twice: {first}; {second}"
            ))
        })?;
    }
    Ok(())
}

fn derive_mlog_next_purge_seconds(
    log: &astersql_meta_model::TableInfo,
) -> SessionResult<Option<i64>> {
    let expression = log
        .MaterializedViewLog
        .as_ref()
        .map(|info| info.PurgeNext.trim())
        .unwrap_or_default();
    if expression.is_empty() {
        return Ok(None);
    }
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(&format!("select {expression}"), "utf8mb4", "utf8mb4_bin")
        .map_err(|error| session_error("parse MLog purge NEXT", error))?;
    let select = statement
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .ok_or_else(|| SessionError::new("MLog purge NEXT is not a scalar expression"))?;
    let expression = select
        .Fields
        .Fields
        .first()
        .and_then(|field| field.Expr.as_ref())
        .ok_or_else(|| SessionError::new("MLog purge NEXT has no expression"))?;
    let mode = log
        .MaterializedViewLog
        .as_ref()
        .expect("MLog metadata")
        .PurgeScheduleSQLMode;
    super::mview_ddl::mlog_schedule_unix_seconds_with_mode(expression, mode)
}

impl ConcreteSession {
    fn check_mlog_operate_privilege(&self, database: &str, log_name: &str) -> SessionResult<()> {
        let Some(user) = self.login_user.as_deref() else {
            return Ok(());
        };
        let host = self
            .authenticated_host
            .as_deref()
            .or(self.login_host.as_deref())
            .unwrap_or("%");
        if runtime_privilege_handle(&self.domain)
            .Get()
            .RequestVerification(
                &self.active_roles.borrow(),
                user,
                host,
                database,
                log_name,
                "",
                astersql_privilege_privileges::OperateViewPriv,
            )
        {
            Ok(())
        } else {
            Err(SessionError::new(format!(
                "OPERATE VIEW command denied to user '{user}'@'{host}' for table '{log_name}'"
            )))
        }
    }

    pub(super) fn execute_purge_materialized_view_log(
        &self,
        statement: &ast::PurgeMaterializedViewLogStmt,
        automatic: bool,
    ) -> SessionResult<()> {
        if self.state.borrow().transaction.is_some() {
            return Err(SessionError::new(
                "cannot run PURGE MATERIALIZED VIEW LOG in explicit transaction",
            ));
        }
        let target = statement
            .Table
            .as_ref()
            .ok_or_else(|| SessionError::new("PURGE MATERIALIZED VIEW LOG has no base table"))?;
        let database = resolve_mlog_database(&target.Schema.L, &self.current_database())?;
        let base = self
            .resolve_runtime_table(&database, &target.Name.L)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "base table {}.{} does not exist",
                    database, target.Name.O
                ))
            })?;
        let log_name = astersql_meta_model::MaterializedViewLogTableName(&base.Name);
        let log = self
            .resolve_runtime_table(&database, &log_name.L)
            .ok_or_else(|| {
                SessionError::new(format!(
                    "materialized view log does not exist for {}.{}",
                    database, base.Name.O
                ))
            })?;
        if log
            .MaterializedViewLog
            .as_ref()
            .is_none_or(|info| info.BaseTableID != base.ID)
        {
            return Err(SessionError::new(
                "materialized view log metadata does not match base table",
            ));
        }
        self.check_mlog_operate_privilege(&database, &log.Name.L)?;
        let maintenance = ConcreteSession::new(Arc::clone(&self.domain));
        let history = ConcreteSession::new(Arc::clone(&self.domain));
        let method = if automatic { "auto" } else { "manual" };
        let started = std::time::Instant::now();
        let mut job_id = 0_u64;
        let mut history_started = false;
        let mut purged = 0_usize;
        let batch_size = self
            .session_vars
            .GetSystemVar(astersql_sessionctx_vardef::TiDBMLogPurgeBatchSize)
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MLOG_PURGE_BATCH_SIZE);
        let operation = (|| -> SessionResult<()> {
            maintenance.execute("begin pessimistic")?;
            let row = purge_sql_row(
                &maintenance,
                &format!(
                    "select LAST_PURGED_TSO, NEXT_PURGE_UNIX_SECONDS from mysql.tidb_mlog_purge_info where MLOG_ID={} for update nowait",
                    log.ID
                ),
            )?
            .ok_or_else(|| SessionError::new(format!("MLog purge info row missing for ID {}", log.ID)))?;
            let last_purged_tso = row
                .first()
                .filter(|value| !value.eq_ignore_ascii_case("null") && value.as_str() != "<nil>")
                .map(|value| {
                    value.parse::<u64>().map_err(|_| {
                        SessionError::new(format!("invalid MLog purge checkpoint {value:?}"))
                    })
                })
                .transpose()?;
            let locked_next_time = row
                .get(1)
                .filter(|value| !value.eq_ignore_ascii_case("null") && value.as_str() != "<nil>")
                .and_then(|value| value.parse::<i64>().ok());
            job_id = maintenance
                .state
                .borrow()
                .transaction
                .as_ref()
                .ok_or_else(|| SessionError::new("MLog purge transaction is missing"))?
                .StartTS();
            if job_id == 0 {
                return Err(SessionError::new("invalid MLog purge start TSO"));
            }
            let mut safe_tso = job_id;
            let building_jobs = purge_sql_rows(
                &maintenance,
                &format!(
                    "select job_meta, table_ids from mysql.tidb_ddl_job where type={}",
                    astersql_meta_model::group_3::ACTION_CREATE_MATERIALIZED_VIEW
                ),
            )?;
            let mut building_view_ids = Vec::new();
            for row in building_jobs {
                let Some(table_ids) = row.get(1) else {
                    continue;
                };
                if !table_ids
                    .split(',')
                    .any(|id| id.trim().parse::<i64>().ok() == Some(log.ID))
                {
                    continue;
                }
                let Some(job_meta) = row.first() else {
                    continue;
                };
                let job = astersql_meta_model::group_3::Job::decode(job_meta.as_bytes())
                    .map_err(|error| session_error("decode MView DDL job", error))?;
                if job.table_id > 0 {
                    building_view_ids.push(job.table_id);
                }
            }
            if let Some(base_info) = &base.MaterializedViewBase {
                for view_id in &base_info.MViewIDs {
                    let row = purge_sql_row(
                        &maintenance,
                        &format!("select LAST_SUCCESS_READ_TSO from mysql.tidb_mview_refresh_info where MVIEW_ID={view_id}"),
                    )?
                    .ok_or_else(|| SessionError::new(format!("MView refresh info missing for ID {view_id}")))?;
                    let read_tso = row
                        .first()
                        .filter(|value| {
                            !value.eq_ignore_ascii_case("null") && value.as_str() != "<nil>"
                        })
                        .map(|value| {
                            value
                                .parse::<u64>()
                                .map_err(|_| SessionError::new("invalid MView refresh TSO"))
                        })
                        .transpose()?
                        .unwrap_or(0);
                    safe_tso = safe_tso.min(read_tso);
                }
            }
            for view_id in building_view_ids {
                if base
                    .MaterializedViewBase
                    .as_ref()
                    .is_some_and(|info| info.MViewIDs.contains(&view_id))
                {
                    continue;
                }
                if let Some(row) = purge_sql_row(
                    &maintenance,
                    &format!(
                        "select LAST_SUCCESS_READ_TSO from mysql.tidb_mview_refresh_info where MVIEW_ID={view_id}"
                    ),
                )? {
                    let read_tso = row
                        .first()
                        .filter(|value| {
                            !value.eq_ignore_ascii_case("null") && value.as_str() != "<nil>"
                        })
                        .map(|value| {
                            value.parse::<u64>().map_err(|_| {
                                SessionError::new("invalid building MView refresh TSO")
                            })
                        })
                        .transpose()?
                        .unwrap_or(0);
                    safe_tso = safe_tso.min(read_tso);
                }
            }
            let latest_cutoff = purge_sql_row(
                &history,
                &format!("select PURGE_CUTOFF_TSO from mysql.tidb_mlog_purge_hist where MLOG_ID={} and PURGE_CUTOFF_TSO is not null order by PURGE_JOB_ID desc limit 1", log.ID),
            )?
            .and_then(|row| row.first().cloned())
            .filter(|value| !value.eq_ignore_ascii_case("null") && value != "<nil>")
            .map(|value| value.parse::<u64>().map_err(|_| SessionError::new("invalid MLog purge history cutoff")))
            .transpose()?;
            let cutoff_fence = last_purged_tso.unwrap_or(0).max(latest_cutoff.unwrap_or(0));
            let skip_by_cutoff_fence = safe_tso < cutoff_fence;
            let schema = purge_sql_string(&database);
            let table = purge_sql_string(&base.Name.O);
            if !skip_by_cutoff_fence {
                history.execute(&format!(
                "insert into mysql.tidb_mlog_purge_hist (PURGE_JOB_ID, MLOG_ID, BASE_TABLE_SCHEMA, BASE_TABLE_NAME, PURGE_METHOD, PURGE_START_TIME, PURGE_ROWS, PURGE_STATUS, PURGE_CUTOFF_TSO) values ({job_id}, {}, '{schema}', '{table}', '{method}', '{}', 0, 'running', {safe_tso})",
                log.ID, purge_history_time()
            ))?;
                history_started = true;
            }
            if !skip_by_cutoff_fence
                && last_purged_tso.is_none_or(|last| safe_tso > last)
                && safe_tso > 0
            {
                // Go first tries an exact TiFlash count and falls back to an
                // unthrottled scan when planning is unavailable. The KV runtime
                // counts at the same MVCC fence, with a bounded best-effort pass.
                let pending = self.domain.storage().with_storage(|store| {
                    let version = store.CurrentVersion("global").map_err(|error| {
                        session_error("read MLog purge count snapshot version", error)
                    })?;
                    let snapshot = store.GetSnapshot(version);
                    pending_mlog_rows(snapshot.as_ref(), log.ID, last_purged_tso, safe_tso)
                });
                let pending = match pending {
                    Ok(pending) => Some(pending),
                    Err(error) => {
                        super::BgLogger().log(
                            super::LogLevel::Warn,
                            "MLog purge pending-row count failed; using unthrottled batches",
                            [super::LogField::String("error".into(), error.to_string())],
                        );
                        None
                    }
                };
                let ranges = pending
                    .as_ref()
                    .map(|stats| mlog_purge_row_id_ranges(stats, log.ShardRowIDBits))
                    .unwrap_or_default();
                let mut throttle = pending.as_ref().and_then(|pending| {
                    let min_rate = self
                        .session_vars
                        .GetSystemVar(astersql_sessionctx_vardef::TiDBMLogPurgeMinRate)
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(astersql_sessionctx_vardef::DefTiDBMLogPurgeMinRate as f64);
                    let ratio = self
                        .session_vars
                        .GetSystemVar(astersql_sessionctx_vardef::TiDBMLogPurgeRateBudgetRatio)
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(astersql_sessionctx_vardef::DefTiDBMLogPurgeRateBudgetRatio);
                    let now = chrono::Utc::now().timestamp();
                    let schedule = if automatic {
                        derive_mlog_next_purge_seconds(&log).ok().flatten()
                    } else {
                        locked_next_time
                    };
                    if schedule.is_some_and(|seconds| seconds <= now) {
                        return None;
                    }
                    let remaining = schedule.and_then(|seconds| seconds.checked_sub(now));
                    let budget = remaining
                        .filter(|seconds| *seconds > 0)
                        .map(|seconds| std::time::Duration::from_secs(seconds as u64))
                        .unwrap_or(MLOG_PURGE_ADAPTIVE_MAX_BUDGET)
                        .min(MLOG_PURGE_ADAPTIVE_MAX_BUDGET);
                    MLogPurgeThrottlePlan::new(
                        pending.pending_rows,
                        std::time::Instant::now() + budget,
                        min_rate,
                        ratio,
                    )
                });
                let delete_started = std::time::Instant::now();
                let scan_ranges: Vec<Option<MLogRowIDRange>> = if ranges.is_empty() {
                    vec![None]
                } else {
                    ranges.into_iter().map(Some).collect()
                };
                for (range_index, range) in scan_ranges.iter().enumerate() {
                    let mut cursor = None;
                    loop {
                        let cancel = purge_sql_row(&history, &format!(
                        "select CANCEL_REQUEST_TIME from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID={job_id} and MLOG_ID={}", log.ID
                    ))?
                    .ok_or_else(|| SessionError::new("MLog purge history row disappeared"))?;
                        if cancel.first().is_some_and(|value| {
                            value != "<nil>" && !value.eq_ignore_ascii_case("null")
                        }) {
                            return Err(SessionError::new("MLog purge job was canceled"));
                        }
                        history.execute(&format!(
                        "update mysql.tidb_mlog_purge_hist set LAST_HEARTBEAT_TIME='{}' where PURGE_JOB_ID={job_id} and MLOG_ID={}",
                        purge_history_time(), log.ID
                    ))?;
                        let effective_batch_size = throttle
                            .as_ref()
                            .map_or(batch_size, |plan| plan.batch_size(batch_size));
                        let (batch, next_cursor, exhausted) =
                            self.domain.storage().with_storage(|store| {
                                let mut transaction = store.Begin(&[]).map_err(|error| {
                                    session_error("begin MLog purge batch", error)
                                })?;
                                let result = purge_mlog_snapshot_batch_from(
                                    transaction.as_mut(),
                                    log.ID,
                                    last_purged_tso,
                                    safe_tso,
                                    effective_batch_size,
                                    cursor.take(),
                                    *range,
                                )?;
                                transaction
                                    .Commit(&kv::Context::default())
                                    .map_err(|error| {
                                        session_error("commit MLog purge batch", error)
                                    })?;
                                Ok::<_, SessionError>(result)
                            })?;
                        cursor = next_cursor;
                        purged += batch;
                        if batch != 0 {
                            let table = astersql_statistics_handle::StatsTableKey::new(
                                &database,
                                &log.Name.O,
                                log.ID,
                            );
                            self.domain
                                .record_stats_mutation(&table, -(batch as i64), batch as i64)
                                .map_err(|error| {
                                    session_error("record MLog purge statistics", error)
                                })?;
                        }
                        if (batch >= effective_batch_size || range_index + 1 < scan_ranges.len())
                            && let Some(plan) = throttle.as_mut()
                        {
                            let mut sleep = plan.sleep_duration(delete_started, purged);
                            while !sleep.is_zero() {
                                let chunk = sleep.min(std::time::Duration::from_millis(200));
                                std::thread::sleep(chunk);
                                sleep = sleep.saturating_sub(chunk);
                                let cancel = purge_sql_row(
                                    &history,
                                    &format!(
                                        "select CANCEL_REQUEST_TIME from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID={job_id} and MLOG_ID={}",
                                        log.ID
                                    ),
                                )?;
                                if cancel.as_ref().and_then(|row| row.first()).is_some_and(
                                    |value| value != "<nil>" && !value.eq_ignore_ascii_case("null"),
                                ) {
                                    return Err(SessionError::new("MLog purge job was canceled"));
                                }
                            }
                        }
                        let cancel = purge_sql_row(&history, &format!(
                        "select CANCEL_REQUEST_TIME from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID={job_id} and MLOG_ID={}", log.ID
                    ))?
                    .ok_or_else(|| SessionError::new("MLog purge history row disappeared"))?;
                        if cancel.first().is_some_and(|value| {
                            value != "<nil>" && !value.eq_ignore_ascii_case("null")
                        }) {
                            return Err(SessionError::new("MLog purge job was canceled"));
                        }
                        if exhausted {
                            break;
                        }
                    }
                }
                maintenance.execute(&format!(
                    "update mysql.tidb_mlog_purge_info set LAST_PURGED_TSO={safe_tso} where MLOG_ID={}",
                    log.ID
                ))?;
            }
            if automatic {
                let next = derive_mlog_next_purge_seconds(&log)?;
                let next_sql = next.map_or_else(|| "NULL".to_owned(), |value| value.to_string());
                maintenance.execute(&format!(
                    "update mysql.tidb_mlog_purge_info set NEXT_PURGE_UNIX_SECONDS={next_sql} where MLOG_ID={}",
                    log.ID
                ))?;
            }
            maintenance.execute("commit")?;
            Ok(())
        })();
        if let Err(error) = operation {
            let _ = maintenance.execute("rollback");
            let reason = purge_sql_string(&error.to_string());
            if job_id == 0 {
                job_id = self.domain.storage().with_storage(|store| {
                    store
                        .CurrentVersion("global")
                        .map(|version| version.Ver)
                        .map_err(|error| {
                            session_error("allocate failed MLog purge history ID", error)
                        })
                })?;
            }
            if history_started {
                finalize_purge_history(
                    &history,
                    &format!(
                        "update mysql.tidb_mlog_purge_hist set PURGE_STATUS='failed', PURGE_ROWS={purged}, PURGE_DURATION_SEC={}, PURGE_END_TIME='{}', PURGE_FAILED_REASON='{reason}' where PURGE_JOB_ID={job_id}",
                        purge_history_duration(started),
                        purge_history_time()
                    ),
                )?;
            } else if job_id != 0 {
                let schema = purge_sql_string(&database);
                let table = purge_sql_string(&base.Name.O);
                history.execute(&format!(
                    "insert into mysql.tidb_mlog_purge_hist (PURGE_JOB_ID, MLOG_ID, BASE_TABLE_SCHEMA, BASE_TABLE_NAME, PURGE_METHOD, PURGE_START_TIME, PURGE_END_TIME, PURGE_ROWS, PURGE_DURATION_SEC, PURGE_STATUS, PURGE_FAILED_REASON) values ({job_id}, {}, '{schema}', '{table}', '{method}', '{}', '{}', {purged}, {}, 'failed', '{reason}')",
                    log.ID, purge_history_time(), purge_history_time(), purge_history_duration(started)
                ))?;
            }
            return Err(error);
        }
        if history_started {
            if let Err(error) = finalize_purge_history(
                &history,
                &format!(
                    "update mysql.tidb_mlog_purge_hist set PURGE_STATUS='success', PURGE_ROWS={purged}, PURGE_DURATION_SEC={}, PURGE_END_TIME='{}' where PURGE_JOB_ID={job_id}",
                    purge_history_duration(started),
                    purge_history_time()
                ),
            ) {
                self.set_warning(format!(
                    "MLog purge committed but could not finalize history: {error}"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn execute_cancel_materialized_view_job(
        &self,
        statement: &ast::CancelMaterializedViewJobStmt,
    ) -> SessionResult<()> {
        if statement.Tp != ast::CancelMaterializedViewJobType::LogPurge || statement.JobID <= 0 {
            return Err(SessionError::new(
                "invalid materialized view purge job ID or type",
            ));
        }
        let job_id = statement.JobID;
        let maintenance = ConcreteSession::new(Arc::clone(&self.domain));
        if self.login_user.is_some() {
            let target = purge_sql_row(
                &maintenance,
                &format!("select MLOG_ID from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID={job_id} and PURGE_STATUS='running'"),
            )?
            .ok_or_else(|| SessionError::new(format!("cannot cancel materialized view log purge job {job_id}")))?;
            let log_id = target
                .first()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| SessionError::new("invalid MLog purge job target"))?;
            let schema = self.domain.info_schema();
            let log = schema.TableByID(log_id).ok_or_else(|| {
                SessionError::new(format!("cannot resolve materialized view log {log_id}"))
            })?;
            let metadata = log
                .ModelMeta()
                .map_err(|error| session_error("read MLog metadata", error))?;
            if metadata.MaterializedViewLog.is_none() {
                return Err(SessionError::new(format!(
                    "table {log_id} is not a materialized view log"
                )));
            }
            let database = astersql_infoschema::SchemaByTable(schema.as_ref(), log.Meta())
                .ok_or_else(|| {
                    SessionError::new(format!(
                        "cannot resolve schema for materialized view log {log_id}"
                    ))
                })?;
            self.check_mlog_operate_privilege(&database.name.lower, &metadata.Name.L)?;
        }
        let cancellable = purge_sql_row(
            &maintenance,
            &format!("select PURGE_STATUS, CANCEL_REQUEST_TIME from mysql.tidb_mlog_purge_hist where PURGE_JOB_ID={job_id}"),
        )?
        .is_some_and(|row| row.first().is_some_and(|status| status == "running") && row.get(1).is_some_and(|value| value == "<nil>" || value.eq_ignore_ascii_case("null")));
        if !cancellable {
            return Err(SessionError::new(format!(
                "cannot cancel materialized view log purge job {job_id}"
            )));
        }
        let requester = self
            .login_user
            .as_deref()
            .map(|user| {
                let host = self
                    .authenticated_host
                    .as_deref()
                    .or(self.login_host.as_deref())
                    .unwrap_or("");
                let identity = format!("'{user}'@'{host}'");
                format!("'{}'", purge_sql_string(&identity))
            })
            .unwrap_or_else(|| "NULL".to_owned());
        maintenance.execute(&format!(
            "update mysql.tidb_mlog_purge_hist set CANCEL_REQUEST_TIME='{}', CANCEL_REQUESTED_BY={requester} where PURGE_JOB_ID={job_id} and PURGE_STATUS='running' and CANCEL_REQUEST_TIME is null",
            purge_history_time()
        ))?;
        Ok(())
    }
}

/// Execute due purge jobs once. The Domain owns the repeating worker; each
/// job still acquires its purge-info row lock before deleting any log records.
pub(crate) fn run_mlog_purge_tick(domain: &Arc<Domain>, now: i64) -> Result<usize, String> {
    let session = ConcreteSession::new(Arc::clone(domain));
    let rows = purge_sql_rows(
        &session,
        &format!(
            "select MLOG_ID from mysql.tidb_mlog_purge_info where NEXT_PURGE_UNIX_SECONDS <= {now}"
        ),
    )
    .map_err(|error| error.to_string())?;
    let mut completed = 0;
    for row in rows {
        let Some(id) = row.first().and_then(|value| value.parse::<i64>().ok()) else {
            return Err("invalid MLog purge schedule ID".to_owned());
        };
        let schema = domain.info_schema();
        let Some(log) = schema.TableByID(id) else {
            continue;
        };
        let log_meta = log.ModelMeta().map_err(|error| error.to_string())?;
        let Some(log_info) = log_meta.MaterializedViewLog.as_ref() else {
            continue;
        };
        let Some(base) = schema.TableByID(log_info.BaseTableID) else {
            continue;
        };
        let base_meta = base.ModelMeta().map_err(|error| error.to_string())?;
        let database = astersql_infoschema::SchemaByTable(schema.as_ref(), base.Meta())
            .ok_or_else(|| format!("schema missing for MLog base table ID {}", base_meta.ID))?;
        let statement = ast::PurgeMaterializedViewLogStmt {
            Table: Some(ast::TableName {
                Schema: ast::NewCIStr(&database.name.original),
                Name: base_meta.Name.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };
        session
            .execute_purge_materialized_view_log(&statement, true)
            .map_err(|error| error.to_string())?;
        completed += 1;
    }
    Ok(completed)
}

pub fn start_domain_mlog_purge_worker(domain: &Arc<Domain>) -> Result<bool, String> {
    let weak = Arc::downgrade(domain);
    domain
        .start_mlog_purge_worker(std::time::Duration::from_secs(10), move |stop| {
            let Some(domain) = weak.upgrade() else {
                return;
            };
            if stop.load(std::sync::atomic::Ordering::Acquire) || domain.is_closed() {
                return;
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if let Err(error) = run_mlog_purge_tick(&domain, now) {
                super::BgLogger().log(
                    super::LogLevel::Error,
                    "MLog purge worker tick failed",
                    [super::LogField::String("error".into(), error)],
                );
            }
        })
        .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "mlog_purge_test.rs"]
mod tests;

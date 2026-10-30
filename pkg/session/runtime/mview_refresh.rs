// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use super::mview_source::{RefreshResources, definition};
use super::*;
use astersql_executor::adapter::{ExecExecutor, ExecutionContext};
use astersql_executor::builder::TypedRuntimeStats;
use astersql_executor::mview_delta_merge_builder::ReaderSnapshot;
use astersql_executor::mview_refresh_steps::{ObserveStep, StepObserver, new_step_set};
use astersql_parser_ast::RefreshMaterializedViewMode as Mode;
use astersql_planner_core::mview_refresh_builder::MViewBuildOptions;
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant, SystemTime};

fn ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}
fn quoted(value: &str) -> String {
    super::dispatch::quote_argument(value)
}
fn nullable<T: ToString>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".into())
}
pub(super) fn sql(session: &ConcreteSession, statement: &str) -> SessionResult<()> {
    for mut set in session.execute(statement)? {
        let drain: SessionResult<()> = (|| {
            while set.next_row()?.is_some() {}
            Ok(())
        })();
        let close = set.close();
        drain.and(close)?;
    }
    Ok(())
}
fn sql_with_runtime_stats(
    session: &ConcreteSession,
    statement: &str,
    stats: Option<&TypedRuntimeStats>,
) -> SessionResult<()> {
    let Some(stats) = stats else {
        return sql(session, statement);
    };
    stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .GetBasicRuntimeStats(0, true);
    let started = Instant::now();
    let mut returned = 0;
    let result = (|| {
        for mut set in session.execute(statement)? {
            let drain: SessionResult<()> = (|| {
                while set.next_row()?.is_some() {
                    returned += 1;
                }
                Ok(())
            })();
            let close = set.close();
            drain.and(close)?;
        }
        Ok(())
    })();
    stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .GetBasicRuntimeStats(0, false)
        .expect("registered write stats")
        .Record(started.elapsed(), returned);
    result
}
fn step_with_write_plan(
    observer: &mut Option<&mut dyn StepObserver>,
    step: &ObserveStep,
    plan: Option<Vec<Vec<String>>>,
    session: &ConcreteSession,
    statement: &str,
) -> SessionResult<()> {
    let stats = observer.as_ref().map(|_| new_refresh_runtime_stats());
    step_with_runtime_plan(observer, step, plan, stats.as_ref(), || {
        sql_with_runtime_stats(session, statement, stats.as_ref())
    })
}
fn rows(session: &ConcreteSession, statement: &str) -> SessionResult<Vec<Vec<String>>> {
    let mut sets = session.execute(statement)?;
    let mut set = sets
        .pop()
        .ok_or_else(|| SessionError::new("refresh query returned no record set"))?;
    let mut rows = Vec::new();
    let drain: SessionResult<()> = (|| {
        while let Some(row) = set.next_row()? {
            rows.push(row);
        }
        Ok(())
    })();
    let close = set.close();
    drain.and(close)?;
    Ok(rows)
}
fn optional_u64(value: Option<&String>) -> SessionResult<Option<u64>> {
    value
        .filter(|value| !matches!(value.to_ascii_lowercase().as_str(), "null" | "<nil>"))
        .map(|value| {
            value
                .parse()
                .map_err(|e| session_error("decode refresh watermark", e))
        })
        .transpose()
}
fn now_sql() -> String {
    quoted(
        &chrono::Utc::now()
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string(),
    )
}
fn fault(name: &str) -> SessionResult<()> {
    if let Some(message) = astersql_testkit_testfailpoint::eval_string(&format!(
        "github.com/pingcap/tidb/pkg/executor/{name}"
    )) {
        return Err(SessionError::new(message));
    }
    Ok(())
}
fn step<T>(
    observer: &mut Option<&mut dyn StepObserver>,
    step: &ObserveStep,
    run: impl FnOnce() -> SessionResult<T>,
) -> SessionResult<T> {
    step_with_plan(observer, step, None, run)
}

fn step_with_plan<T>(
    observer: &mut Option<&mut dyn StepObserver>,
    step: &ObserveStep,
    plan_rows: Option<Vec<Vec<String>>>,
    run: impl FnOnce() -> SessionResult<T>,
) -> SessionResult<T> {
    step_with_runtime_plan(observer, step, plan_rows, None, run)
}
fn step_with_runtime_plan<T>(
    observer: &mut Option<&mut dyn StepObserver>,
    step: &ObserveStep,
    plan_rows: Option<Vec<Vec<String>>>,
    stats: Option<&TypedRuntimeStats>,
    run: impl FnOnce() -> SessionResult<T>,
) -> SessionResult<T> {
    if let Some(observer) = observer.as_deref_mut() {
        observer.on_step_start(step, SystemTime::now());
    }
    let result = run();
    if let (Some(observer), Some(rows)) = (observer.as_deref_mut(), plan_rows) {
        observer.on_step_plan_rows(
            step,
            if let Some(stats) = stats {
                analyze_refresh_plan_rows(rows, stats)
            } else {
                rows
            },
        );
    }
    let error = result.as_ref().err().map(ToString::to_string);
    if let Some(observer) = observer.as_deref_mut() {
        observer.on_step_end(step, SystemTime::now(), error.as_deref());
    }
    result
}

fn timed_step<T>(
    duration: &mut Duration,
    observer: &mut Option<&mut dyn StepObserver>,
    current: &ObserveStep,
    run: impl FnOnce() -> SessionResult<T>,
) -> SessionResult<T> {
    timed_step_with_plan(duration, observer, current, None, None, run)
}
fn timed_step_with_plan<T>(
    duration: &mut Duration,
    observer: &mut Option<&mut dyn StepObserver>,
    current: &ObserveStep,
    plan: Option<Vec<Vec<String>>>,
    stats: Option<&TypedRuntimeStats>,
    run: impl FnOnce() -> SessionResult<T>,
) -> SessionResult<T> {
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        step_with_runtime_plan(observer, current, plan, stats, run)
    }));
    *duration = started.elapsed();
    match result {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[derive(Default)]
pub(super) struct InternalCollectors {
    pub stats: Option<Arc<astersql_statistics_handle_usage::SessionStatsItem>>,
    pub indexes: Option<Arc<astersql_statistics_handle_usage::SessionIndexUsageCollector>>,
}
fn collect_execution_info() -> bool {
    astersql_config::get_global_config()
        .instance
        .enable_collect_execution_info
        .load()
}
/// Nested maintenance helpers retain pre-existing collector identities and
/// detach only the entries installed by their own scope.
pub(super) struct InternalCollectorGuard {
    session: ConcreteSession,
    stats: bool,
    indexes: bool,
}
impl InternalCollectorGuard {
    pub(super) fn attach(session: &ConcreteSession) -> Self {
        let mut current = session.internal_collectors.borrow_mut();
        let stats = session.domain.stats_updating() && current.stats.is_none();
        let indexes = session.domain.stats_updating()
            && collect_execution_info()
            && current.indexes.is_none();
        if stats {
            current.stats = Some(Arc::new(session.domain.internal_session_stats().new_item()));
        }
        if indexes {
            current.indexes = Some(Arc::new(
                session.domain.internal_index_usage().spawn_session(),
            ));
        }
        Self {
            session: session.clone(),
            stats,
            indexes,
        }
    }
}
impl Drop for InternalCollectorGuard {
    fn drop(&mut self) {
        let mut current = self.session.internal_collectors.borrow_mut();
        if self.stats {
            if let Some(stats) = current.stats.take() {
                stats.delete();
            }
        }
        if self.indexes {
            if let Some(indexes) = current.indexes.take() {
                indexes.flush();
            }
        }
        drop(current);
        self.session.domain.flush_internal_session_stats();
    }
}

/// Real elapsed durations, also exposed to clock-boundary tests without sleeps.
#[derive(Default, Clone, Debug)]
pub struct RefreshTimings {
    pub total: Duration,
    pub execution: Duration,
    pub lock_refresh_info: Duration,
    pub transaction_total: Duration,
    pub create_shadow: Duration,
    pub cutover: Duration,
    pub cleanup_shadow: Duration,
    pub out_of_place: bool,
    pub success: bool,
}
impl RefreshTimings {
    pub fn overhead(&self) -> Duration {
        self.total.saturating_sub(self.execution)
    }
    pub fn should_log(&self) -> bool {
        self.overhead() >= Duration::from_secs(5)
    }
    pub(super) fn emit(&self, table: &astersql_meta_model::TableInfo, database: &str) {
        if !self.should_log() {
            return;
        }
        let mut fields = vec![
            LogField::String("schema".into(), database.into()),
            LogField::String("mview".into(), table.Name.O.clone()),
            LogField::I64("mviewID".into(), table.ID),
            LogField::U64(
                "total".into(),
                self.total.as_nanos().min(u64::MAX as u128) as u64,
            ),
            LogField::U64(
                if self.out_of_place {
                    "loadShadow"
                } else {
                    "executeRefreshMaterializedViewDataChanges"
                }
                .into(),
                self.execution.as_nanos().min(u64::MAX as u128) as u64,
            ),
            LogField::U64(
                "overhead".into(),
                self.overhead().as_nanos().min(u64::MAX as u128) as u64,
            ),
            LogField::U64("slowOverheadThreshold".into(), 5_000_000_000),
            LogField::Bool("success".into(), self.success),
        ];
        if self.out_of_place {
            for (name, duration) in [
                ("createShadow", self.create_shadow),
                ("cutover", self.cutover),
                ("cleanupShadow", self.cleanup_shadow),
            ] {
                fields.push(LogField::U64(
                    name.into(),
                    duration.as_nanos().min(u64::MAX as u128) as u64,
                ));
            }
        } else {
            fields.push(LogField::U64(
                "lockRefreshInfoRow".into(),
                self.lock_refresh_info.as_nanos().min(u64::MAX as u128) as u64,
            ));
            fields.push(LogField::U64(
                "transactionTotal".into(),
                self.transaction_total.as_nanos().min(u64::MAX as u128) as u64,
            ));
        }
        BgLogger().log(
            LogLevel::Info,
            if self.out_of_place {
                "refresh materialized view complete out-of-place is slow"
            } else {
                "refresh materialized view is slow"
            },
            fields,
        );
    }
}

/// Utility-style runtime. This source commit intentionally exposes a direct
/// constructor before the public SQL builder arrives in the following port.
pub struct RefreshMaterializedViewExec {
    session: ConcreteSession,
    statement: ast::RefreshMaterializedViewStmt,
    automatic: bool,
    done: bool,
    pub timings: RefreshTimings,
    pub timing_override: Option<Box<dyn Fn(Duration, Duration) -> (Duration, Duration)>>,
}
impl RefreshMaterializedViewExec {
    pub fn new(
        session: ConcreteSession,
        statement: ast::RefreshMaterializedViewStmt,
        automatic: bool,
    ) -> Self {
        Self {
            session,
            statement,
            automatic,
            done: false,
            timings: Default::default(),
            timing_override: None,
        }
    }
    pub fn next(&mut self, mut observer: Option<&mut dyn StepObserver>) -> SessionResult<()> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.execute(&mut observer)
    }
    fn execute(&mut self, observer: &mut Option<&mut dyn StepObserver>) -> SessionResult<()> {
        if self.session.sql_dialect() != ast::SqlDialect::MySQL {
            return Err(SessionError::new(
                "materialized view refresh requires the MySQL dialect",
            ));
        }
        let statement = self.statement.clone();
        let mode = statement.mode().map_err(SessionError::new)?;
        let target = statement
            .ViewName
            .as_ref()
            .ok_or_else(|| SessionError::new("refresh materialized view: missing view name"))?;
        if statement.WithAsyncMode {
            return Err(SessionError::new(
                "refresh materialized view: WITH ASYNC MODE is not supported yet",
            ));
        }
        if statement.AsOf.is_some() && mode != Mode::Fast {
            return Err(SessionError::new(
                "refresh materialized view: AS OF TIMESTAMP is only supported for FAST refresh",
            ));
        }
        let database = if target.Schema.L.is_empty() {
            self.session.current_database()
        } else {
            target.Schema.O.clone()
        };
        if database.is_empty() {
            return Err(SessionError::new("No database selected"));
        }
        let table = self
            .session
            .resolve_runtime_table(&database, &target.Name.L)
            .ok_or_else(|| SessionError::new("refresh materialized view target does not exist"))?;
        let info = table
            .MaterializedView
            .as_ref()
            .ok_or_else(|| SessionError::new("target is not a materialized view"))?;
        if !info.GetInitBuildState().IsReady() {
            return Err(SessionError::new(
                info.GetInitBuildState()
                    .AccessErrorMessage(&format!("{database}.{}", table.Name.O)),
            ));
        }
        if info.SQLContent.is_empty() {
            return Err(SessionError::new(
                "refresh materialized view: invalid select sql",
            ));
        }
        self.session.check_refresh_base_select(info)?;
        let maintenance = ConcreteSession::new(self.session.domain.clone());
        let history = ConcreteSession::new(self.session.domain.clone());
        let _collectors = InternalCollectorGuard::attach(&maintenance);
        maintenance.state.borrow_mut().in_restricted_sql = true;
        history.state.borrow_mut().in_restricted_sql = true;
        sql(&maintenance, &format!("USE {}", ident(&database)))?;
        let target_ts = statement
            .AsOf
            .as_ref()
            .map(|asof| self.session.evaluate_refresh_target_ts(&asof.TsExpr))
            .transpose()?
            .unwrap_or(0);
        let steps = new_step_set(mode);
        let name = match mode {
            Mode::Fast if target_ts > 0 => "bounded fast",
            Mode::Fast => "fast",
            Mode::CompleteInPlace => "complete in place",
            Mode::CompleteDeltaApply => "complete delta apply",
            Mode::CompleteOutOfPlace => "complete out of place",
        };
        let method = format!("{name} {}", if self.automatic { "auto" } else { "manual" });
        let started = Instant::now();
        let started_at = now_sql();
        let lock_name = format!("mv_refresh_{}_{}", table.DBID, table.ID);
        let mut job_id = 0;
        let mut history_started = false;
        let mut refresh_result = (0_u64, String::new());
        let mut cancellation = None;
        let mut committed = false;
        let mut acquired_lock = false;
        let mut initialization = None;
        let mut precision = None;
        let mut watermark = None;
        let mut transaction_started = None;
        let mut shadow = ShadowGuard::new(maintenance.clone(), database.clone(), table.clone());
        self.timings = RefreshTimings {
            out_of_place: mode == Mode::CompleteOutOfPlace,
            ..Default::default()
        };
        let operation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> SessionResult<()> {
                initialization = Some(initialize_session(
                    &maintenance,
                    info,
                    &self.session,
                    self.automatic,
                )?);
                precision = Some(super::relational_value::BuildDivisionPrecision::enter(
                    info.DefinitionDivPrecisionIncrement,
                )?);
                let lock = rows(
                    &maintenance,
                    &format!("SELECT GET_LOCK({},1)", quoted(&lock_name)),
                )?;
                if lock
                    .first()
                    .and_then(|row| row.first())
                    .is_none_or(|value| value != "1")
                {
                    return Err(SessionError::new(format!(
                        "another refresh is running for materialized view {database}.{}, please retry later",
                        table.Name.O
                    )));
                }
                acquired_lock = true;
                fault("mockRefreshMaterializedViewErrorBeforeInsertHist")?;
                let locked = if mode == Mode::CompleteOutOfPlace {
                    read_refresh_info(&maintenance, table.ID, false)?
                } else {
                    let transaction_start = Instant::now();
                    step(observer, &steps[0], || {
                        sql(&maintenance, "BEGIN PESSIMISTIC")?;
                        transaction_started = Some(transaction_start);
                        Ok(())
                    })?;
                    let locked = timed_step(
                        &mut self.timings.lock_refresh_info,
                        observer,
                        &steps[1],
                        || read_refresh_info(&maintenance, table.ID, true),
                    )?;
                    locked
                };
                watermark = locked.0;
                if mode == Mode::Fast && watermark.is_none() {
                    return Err(SessionError::new(
                        "refresh materialized view fast: LAST_SUCCESS_READ_TSO is NULL",
                    ));
                }
                if mode == Mode::Fast && target_ts > 0 && watermark == Some(target_ts) {
                    return Ok(());
                }
                let read_ts = maintenance
                    .state
                    .borrow()
                    .transaction
                    .as_ref()
                    .map(|tx| tx.StartTS())
                    .map(Ok)
                    .unwrap_or_else(|| allocate_tso(&maintenance))?;
                job_id = read_ts;
                let hist_index = if mode == Mode::CompleteOutOfPlace {
                    0
                } else {
                    2
                };
                step(observer, &steps[hist_index], || {
                    insert_running(
                        &history,
                        job_id,
                        table.ID,
                        &database,
                        &table.Name.O,
                        &method,
                        &started_at,
                    )
                })?;
                history_started = true;
                let mut monitor = TaskMonitor::start(
                    self.session.domain.clone(),
                    job_id,
                    table.ID,
                    maintenance.sql_killer.clone(),
                );
                cancellation = Some(monitor.cancellation.clone());
                let execution_start = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if mode == Mode::CompleteOutOfPlace {
                        self.out_of_place(
                            observer,
                            &steps,
                            &maintenance,
                            &table,
                            &database,
                            read_ts,
                            watermark,
                            &mut shadow,
                            &mut committed,
                            &monitor,
                        )
                    } else {
                        self.in_place(
                            observer,
                            &steps,
                            &maintenance,
                            &history,
                            &table,
                            &database,
                            mode,
                            read_ts,
                            watermark,
                            target_ts,
                        )
                    }
                }));
                if mode != Mode::CompleteOutOfPlace {
                    self.timings.execution = execution_start.elapsed();
                }
                match result {
                    Ok(result) => result?,
                    Err(payload) => std::panic::resume_unwind(payload),
                };
                monitor.check_cancelled()?;
                refresh_result = (
                    maintenance.session_vars.StmtCtx.AffectedRows(),
                    maintenance.session_vars.StmtCtx.GetMessage(),
                );
                monitor.stop();
                if mode != Mode::CompleteOutOfPlace {
                    fault("mockRefreshMaterializedViewErrorAfterDataChanges")?;
                    let persisted = if mode == Mode::Fast && target_ts > 0 {
                        if target_ts > read_ts {
                            return Err(SessionError::new(
                                "refresh materialized view fast as of timestamp: target tso is newer than actual refresh read tso",
                            ));
                        }
                        target_ts
                    } else {
                        read_ts
                    };
                    step(observer, &steps[steps.len() - 3], || {
                        persist_success(
                            &maintenance,
                            table.ID,
                            watermark,
                            persisted,
                            self.next_schedule(info)?,
                        )
                    })?;
                    step(observer, &steps[steps.len() - 2], || {
                        sql(&maintenance, "COMMIT")?;
                        committed = true;
                        if let Some(started) = transaction_started.take() {
                            self.timings.transaction_total = started.elapsed();
                        }
                        Ok(())
                    })?;
                }
                let schedule = if self.automatic {
                    locked.1.map(|previous| {
                        Duration::from_secs(
                            chrono::Utc::now()
                                .timestamp()
                                .saturating_sub(previous)
                                .max(0) as u64,
                        )
                        .saturating_sub(started.elapsed())
                    })
                } else {
                    None
                };
                if let Some(duration) = schedule {
                    astersql_metrics::MVServiceRefreshScheduleDurationHistogram()
                        .observe(duration.as_secs_f64());
                }
                let final_read = if mode == Mode::CompleteOutOfPlace {
                    shadow.read_ts.get()
                } else if target_ts > 0 {
                    target_ts
                } else {
                    read_ts
                };
                if let Err(error) = step(observer, steps.last().unwrap(), || {
                    finalize_history(
                        &history,
                        job_id,
                        table.ID,
                        "success",
                        Some(final_read),
                        if mode == Mode::CompleteOutOfPlace {
                            None
                        } else {
                            Some(maintenance.state.borrow().last_commit_ts)
                        },
                        &started_at,
                        started.elapsed(),
                        schedule,
                        maintenance
                            .mview_scan
                            .borrow()
                            .as_ref()
                            .map(|(_, rows)| rows.load(Ordering::Relaxed)),
                        None,
                    )
                }) {
                    self.session
                        .state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::warning(format!(
                            "refresh committed but failed to finalize refresh history: {error}"
                        )));
                }
                if let Err(error) = sql(
                    &history,
                    &format!(
                        "DELETE FROM mysql.tidb_mview_refresh_alert WHERE MVIEW_ID={}",
                        table.ID
                    ),
                ) {
                    self.session
                        .state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::warning(error.to_string()));
                }
                let affected = refresh_result.0;
                self.session.session_vars.StmtCtx.SetAffectedRows(affected);
                self.session
                    .session_vars
                    .StmtCtx
                    .SetMessage(refresh_result.1.clone());
                let mut report = maintenance.LastDmlReport().unwrap_or_default();
                report.AffectedRows = affected;
                report.Committed = true;
                self.session.state.borrow_mut().last_dml_report = Some(report);
                astersql_metrics::executor::RecordRefreshAffectedRows(affected);
                Ok(())
            },
        ));
        let mut result = match operation {
            Ok(result) => result,
            Err(payload) if payload.is::<SessionError>() => {
                Err(*payload.downcast::<SessionError>().unwrap())
            }
            Err(payload) => {
                let message = if let Some(message) = payload.downcast_ref::<String>() {
                    message.clone()
                } else if let Some(message) = payload.downcast_ref::<&str>() {
                    message.to_string()
                } else {
                    "materialized view refresh panicked".into()
                };
                Err(SessionError::new(message))
            }
        };
        if !committed {
            if let Err(rollback) = sql(&maintenance, "ROLLBACK") {
                result = Err(match result {
                    Err(error) => SessionError::with_source(
                        format!("{error}; rollback error: {rollback}"),
                        astersql_errors::SharedError::new(error),
                    ),
                    Ok(()) => rollback,
                });
            }
        }
        if let Some(started) = transaction_started.take() {
            self.timings.transaction_total = started.elapsed();
        }
        if shadow.armed.get() {
            if let Err(cleanup) = shadow.cleanup() {
                if let Err(error) = &result {
                    result = Err(SessionError::with_source(
                        format!("{error}; shadow cleanup failed: {cleanup}"),
                        astersql_errors::SharedError::new(error.clone()),
                    ));
                } else {
                    result = Err(cleanup);
                }
            }
        }
        self.timings.cleanup_shadow = shadow.cleanup_duration.get();
        if result.is_err() {
            let manual =
                cancellation
                    .as_ref()
                    .and_then(|state: &Arc<std::sync::Mutex<Option<String>>>| {
                        state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    });
            let reason = manual.unwrap_or_else(|| result.as_ref().unwrap_err().to_string());
            if cancellation.as_ref().is_some_and(|state| {
                state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some()
            }) {
                result = Err(SessionError::new(
                    "materialized view task canceled manually",
                ));
            }
            if job_id == 0 {
                job_id = allocate_tso(&history)?;
            }
            let finalized = if history_started {
                step(observer, steps.last().unwrap(), || {
                    finalize_history(
                        &history,
                        job_id,
                        table.ID,
                        "failed",
                        (target_ts > 0).then_some(target_ts),
                        None,
                        &started_at,
                        started.elapsed(),
                        None,
                        None,
                        Some(&reason),
                    )
                })
            } else {
                insert_failed(
                    &history,
                    job_id,
                    table.ID,
                    &database,
                    &table.Name.O,
                    &method,
                    &started_at,
                    started.elapsed(),
                    (target_ts > 0).then_some(target_ts),
                    &reason,
                )
            };
            if let Err(error) = finalized {
                result = Err(SessionError::new(format!(
                    "{reason}; refresh history finalization failed: {error}"
                )));
            }
            if info.AlertRefreshFailed {
                let _ = sql(
                    &history,
                    &format!(
                        "INSERT INTO mysql.tidb_mview_refresh_alert (MVIEW_ID,MVIEW_SCHEMA,MVIEW_NAME,REFRESH_FAILED,UPDATE_TIME) VALUES ({},{},{},'YES',NOW(6)) ON DUPLICATE KEY UPDATE REFRESH_FAILED='YES',UPDATE_TIME=NOW(6)",
                        table.ID,
                        quoted(&database),
                        quoted(&table.Name.O)
                    ),
                );
            }
        }
        if acquired_lock {
            match rows(
                &maintenance,
                &format!("SELECT RELEASE_LOCK({})", quoted(&lock_name)),
            ) {
                Ok(rows)
                    if rows
                        .first()
                        .and_then(|row| row.first())
                        .is_some_and(|value| value == "1") => {}
                Ok(_) if result.is_ok() => {
                    result = Err(SessionError::new(
                        "refresh advisory lock cleanup invariant violated",
                    ))
                }
                Err(error) if result.is_ok() => result = Err(error),
                _ => {}
            }
        }
        self.timings.total = started.elapsed();
        self.timings.success = result.is_ok();
        if let Some(clock) = &self.timing_override {
            let (total, execution) = clock(self.timings.total, self.timings.execution);
            self.timings.total = total;
            self.timings.execution = execution;
        }
        self.timings.emit(&table, &database);
        result
    }
    fn next_schedule(
        &self,
        info: &astersql_meta_model::MaterializedViewInfo,
    ) -> SessionResult<Option<Option<i64>>> {
        if !self.automatic {
            return Ok(None);
        }
        if info.RefreshNext.trim().is_empty() {
            return Ok(Some(None));
        }
        let node = astersql_parser::New()
            .ParseOneStmt(&format!("SELECT {}", info.RefreshNext), "", "")
            .map_err(|e| session_error("parse refresh NEXT", e))?;
        let expression = node
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .and_then(|select| select.Fields.Fields.first())
            .and_then(|field| field.Expr.as_ref())
            .ok_or_else(|| SessionError::new("refresh NEXT expression is missing"))?;
        super::mview_ddl::mlog_schedule_unix_seconds_with_mode(
            expression,
            info.RefreshScheduleSQLMode,
        )
        .map(Some)
    }
    fn in_place(
        &self,
        observer: &mut Option<&mut dyn StepObserver>,
        steps: &[ObserveStep],
        maintenance: &ConcreteSession,
        history: &ConcreteSession,
        table: &astersql_meta_model::TableInfo,
        database: &str,
        mode: Mode,
        read_ts: u64,
        watermark: Option<u64>,
        target_ts: u64,
    ) -> SessionResult<()> {
        let qualified = format!("{}.{}", ident(database), ident(&table.Name.O));
        if mode == Mode::CompleteInPlace {
            let delete_plan = if observer.is_some() {
                Some(
                    maintenance
                        .refresh_write_plan("Delete", &format!("SELECT * FROM {qualified}"))?,
                )
            } else {
                None
            };
            step_with_write_plan(
                observer,
                &steps[3],
                delete_plan,
                maintenance,
                &format!("DELETE FROM {qualified}"),
            )?;
            let deleted = maintenance.protocol_state().affected_rows;
            let insert_plan = if observer.is_some() {
                Some(maintenance.refresh_write_plan(
                    "Insert",
                    &table.MaterializedView.as_ref().unwrap().SQLContent,
                )?)
            } else {
                None
            };
            step_with_write_plan(
                observer,
                &steps[4],
                insert_plan,
                maintenance,
                &format!(
                    "INSERT INTO {qualified} {}",
                    table.MaterializedView.as_ref().unwrap().SQLContent
                ),
            )?;
            let inserted = maintenance.protocol_state().affected_rows;
            maintenance
                .session_vars
                .StmtCtx
                .SetAffectedRows(inserted + deleted);
            maintenance.session_vars.StmtCtx.SetMessage(format!(
                "Rows inserted: {inserted}  Updated: 0  Deleted: {deleted}"
            ));
            return Ok(());
        }
        let (definition, select, _) = definition(maintenance, database, table)?;
        if mode == Mode::Fast {
            if target_ts > 0 && target_ts < watermark.unwrap_or(0) {
                return Err(SessionError::new(
                    "refresh materialized view fast as of timestamp: target tso is older than LAST_SUCCESS_READ_TSO",
                ));
            }
            check_log_integrity(history, &definition, watermark.unwrap_or(0))?;
        }
        let mut handles = Vec::new();
        let mut delta = None;
        let mut complete = None;
        let source = if mode == Mode::Fast {
            let log = maintenance
                .domain
                .info_schema()
                .TableByID(definition.mlog_table_id)
                .ok_or_else(|| SessionError::new("materialized view log does not exist"))?
                .ModelMeta()
                .map_err(|e| session_error("read MLog metadata", e))?;
            let (source, plan) =
                astersql_planner_core::mview_refresh_source::BuildFastRefreshSource(
                    &definition,
                    &table.Name.O,
                    &log.Name.O,
                    &select,
                    MViewBuildOptions {
                        from_ts: watermark.unwrap_or(0),
                        to_ts: target_ts,
                    },
                )
                .map_err(SessionError::new)?;
            handles = plan.mv_table_pk_cols.clone();
            delta = Some(plan);
            source
        } else {
            let (source, plan) =
                astersql_planner_core::mview_refresh_source::BuildCompleteRefreshSource(
                    &definition,
                    &table.Name.O,
                    *select,
                )
                .map_err(SessionError::new)?;
            handles = plan.current_handle_cols.clone();
            complete = Some(plan);
            source
        };
        if mode == Mode::Fast && collect_execution_info() {
            *maintenance.mview_scan.borrow_mut() =
                Some((definition.mlog_table_id, Arc::new(AtomicU64::new(0))));
        }
        let runtime_stats = observer.as_ref().map(|_| new_refresh_runtime_stats());
        let child = maintenance
            .compile_mview_source_with_stats(source, read_ts, runtime_stats.as_ref())
            .map_err(|e| SessionError::new(e.to_string()))?;
        let mut source_plan_rows = vec![vec![
            if mode == Mode::Fast {
                "MViewDeltaMerge_0"
            } else {
                "MViewCompleteDeltaApply_0"
            }
            .into(),
        ]];
        source_plan_rows.extend(maintenance.mview_plan_rows.borrow().clone());
        let slow_log =
            super::typed_adapter_bridge::SessionBoundAdapterOwner::new(maintenance.clone());
        slow_log.effects.borrow_mut().process_started = Some(SystemTime::now());
        let implementation = ast::RefreshMaterializedViewImplementStmt {
            RefreshStmt: Some(self.statement.clone()),
            LastSuccessfulRefreshReadTSO: watermark.unwrap_or(0),
            TargetRefreshReadTSO: target_ts,
            ..Default::default()
        };
        let slow_text =
            astersql_executor::adapter::restoreStmtTextForSlowLogWhenEmptySQL(&implementation);
        let resources = Arc::new(RefreshResources {
            session: maintenance.clone(),
            database: database.into(),
            table: table.clone(),
            definition,
            handles,
            read_ts: if target_ts > 0 { target_ts } else { read_ts },
            statement: Arc::from(astersql_sessionctx_stmtctx::NewStmtCtx()),
        });
        let statement_context = resources.statement.clone();
        let mut executor = if let Some(plan) = delta {
            let reader = ReaderSnapshot {
                ts: if target_ts > 0 { target_ts } else { read_ts },
                snapshot: std::sync::Mutex::new(Box::new(KvRefreshSnapshot(
                    maintenance.domain.storage().with_storage(|store| {
                        store.GetSnapshot(kv::Version {
                            Ver: if target_ts > 0 { target_ts } else { read_ts },
                        })
                    }),
                ))),
                info_schema: Arc::new(RefreshSchema(maintenance.domain.info_schema())),
            };
            astersql_executor::mview_delta_merge_builder::build(&plan, child, resources, reader)
        } else {
            astersql_executor::mview_complete_delta_apply_builder::build(
                complete.as_ref().unwrap(),
                child,
                resources,
            )
        }
        .map_err(|e| SessionError::new(e.to_string()))?;
        let mut executor =
            astersql_executor::builder::observe_typed_executor(executor, 0, runtime_stats.as_ref());
        step_with_runtime_plan(
            observer,
            &steps[3],
            Some(source_plan_rows),
            runtime_stats.as_ref(),
            || {
                executor
                    .Open()
                    .map_err(|e| SessionError::new(e.to_string()))?;
                let mut output = executor.NewChunk();
                let result = executor
                    .NextWithContext(
                        &ExecutionContext {
                            sql_killer: Some(maintenance.sql_killer.clone()),
                            ..Default::default()
                        },
                        &mut output,
                    )
                    .map_err(|e| SessionError::new(e.to_string()));
                let close = executor
                    .Close()
                    .map_err(|e| SessionError::new(e.to_string()));
                let result = result.and(close);
                astersql_executor::adapter::AdapterRuntime::SlowQuery(
                    &slow_log,
                    read_ts,
                    &slow_text,
                    result.is_ok(),
                    false,
                );
                if result.is_ok() {
                    maintenance
                        .session_vars
                        .StmtCtx
                        .SetAffectedRows(statement_context.AffectedRows());
                    maintenance
                        .session_vars
                        .StmtCtx
                        .SetMessage(statement_context.GetMessage());
                }
                result
            },
        )
    }
    fn out_of_place(
        &mut self,
        observer: &mut Option<&mut dyn StepObserver>,
        steps: &[ObserveStep],
        maintenance: &ConcreteSession,
        table: &astersql_meta_model::TableInfo,
        database: &str,
        read_ts: u64,
        watermark: Option<u64>,
        shadow: &mut ShadowGuard,
        committed: &mut bool,
        monitor: &TaskMonitor,
    ) -> SessionResult<()> {
        timed_step(&mut self.timings.create_shadow, observer, &steps[1], || {
            shadow.create()
        })?;
        let load_plan = if observer.is_some() {
            let store = maintenance
                .domain
                .storage()
                .with_storage(|store| store.Name());
            Some(maintenance.refresh_write_plan(
                if store == "TiKV" {
                    "ImportInto"
                } else {
                    "Insert"
                },
                &table.MaterializedView.as_ref().unwrap().SQLContent,
            )?)
        } else {
            None
        };
        let load_stats = observer.as_ref().map(|_| new_refresh_runtime_stats());
        let build = timed_step_with_plan(
            &mut self.timings.execution,
            observer,
            &steps[2],
            load_plan,
            load_stats.as_ref(),
            || {
                let source = &table.MaterializedView.as_ref().unwrap().SQLContent;
                if maintenance.state.borrow().transaction.is_some() {
                    return Err(SessionError::new(
                        "refresh materialized view complete OUT OF PLACE: build session unexpectedly in transaction",
                    ));
                }
                let store = maintenance
                    .domain
                    .storage()
                    .with_storage(|store| store.Name());
                let sql_text = build_shadow_sql(
                    database,
                    &shadow.name,
                    table,
                    &store,
                    maintenance
                        .session_vars
                        .GetSystemVar("tidb_mview_maintain_import_threads")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    &maintenance
                        .session_vars
                        .GetSystemVar("tidb_mview_maintain_import_disk_quota")
                        .unwrap_or_default(),
                )?;
                sql_with_runtime_stats(maintenance, &sql_text, load_stats.as_ref())?;
                let inserted = maintenance.protocol_state().affected_rows;
                let info: serde_json::Value =
                    serde_json::from_str(&maintenance.state.borrow().last_query_info)
                        .map_err(|e| session_error("read shadow build query info", e))?;
                let actual_read = info
                .get("start_ts")
                .and_then(serde_json::Value::as_u64)
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    SessionError::new(
                        "refresh materialized view complete OUT OF PLACE: invalid build read tso",
                    )
                })?;
                shadow.read_ts.set(actual_read);
                maintenance.session_vars.StmtCtx.SetAffectedRows(inserted);
                maintenance
                    .session_vars
                    .StmtCtx
                    .SetMessage(format!("Rows inserted: {inserted}  Updated: 0  Deleted: 0"));
                Ok(())
            },
        );
        build?;
        let load_result = (
            maintenance.session_vars.StmtCtx.AffectedRows(),
            maintenance.session_vars.StmtCtx.GetMessage(),
        );
        fault("mockRefreshMaterializedViewOutOfPlaceBuildErrorAfterLoadShadow")?;
        let next_schedule = self
            .next_schedule(table.MaterializedView.as_ref().unwrap())?
            .flatten();
        timed_step(&mut self.timings.cutover, observer, &steps[3], || {
            monitor.check_cancelled()?;
            let args=astersql_meta_model::group_2::RefreshMaterializedViewCompleteOutOfPlaceCutoverArgs {OldMViewID:table.ID,ShadowTableID:shadow.id.get(),BuildReadTSO:shadow.read_ts.get(),ExpectedOldMViewRevision:Some(table.Revision),ExpectedLastSuccessReadTSO:watermark.unwrap_or(0),ExpectedLastSuccessReadTSONull:watermark.is_none(),NextRefreshUnixSeconds:next_schedule,ShouldUpdateNextRefreshUnixSeconds:self.automatic};
            maintenance.submit_normal_action(
                database,
                &table.Name.O,
                92,
                serde_json::to_value(args)
                    .map_err(|e| session_error("encode refresh cutover", e))?,
            )?;
            shadow.armed.set(false);
            *committed = true;
            maintenance
                .domain
                .refresh_materialized_view_metadata()
                .map_err(|error| session_error("publish MV cutover metadata", error))?;
            Ok(())
        })?;
        maintenance
            .session_vars
            .StmtCtx
            .SetAffectedRows(load_result.0);
        maintenance.session_vars.StmtCtx.SetMessage(load_result.1);
        let _ = read_ts;
        Ok(())
    }
}
pub(super) fn drop_shadow_table(
    session: &ConcreteSession,
    database: &str,
    table: &str,
) -> SessionResult<()> {
    session.submit_normal_action(database, table, 94, drop_shadow_args(database, table)?)?;
    session
        .domain
        .refresh_materialized_view_metadata()
        .map_err(|error| session_error("publish shadow drop metadata", error))?;
    Ok(())
}

pub(super) fn drop_shadow_args(database: &str, table: &str) -> SessionResult<serde_json::Value> {
    use astersql_meta_model::group_2::{DropTableArgs, ast as model_ast};
    serde_json::to_value(DropTableArgs {
        Identifiers: vec![model_ast::Ident {
            Schema: model_ast::NewCIStr(database),
            Name: model_ast::NewCIStr(table),
        }],
        ..Default::default()
    })
    .map_err(|error| session_error("encode drop shadow arguments", error))
}

pub(super) fn build_shadow_sql(
    database: &str,
    shadow: &str,
    table: &astersql_meta_model::TableInfo,
    store: &str,
    threads: usize,
    quota: &str,
) -> SessionResult<String> {
    let source = table
        .MaterializedView
        .as_ref()
        .filter(|info| !info.SQLContent.is_empty())
        .ok_or_else(|| SessionError::new("refresh materialized view: invalid select sql"))?;
    Ok(if store == "TiKV" {
        let mut options = vec!["disable_precheck".to_owned()];
        if threads > 0 {
            options.push(format!("thread={threads}"));
        }
        if !quota.is_empty() {
            options.push(format!("disk_quota={}", quoted(quota)));
        }
        format!(
            "IMPORT INTO {}.{} FROM ({}) WITH {}",
            ident(database),
            ident(shadow),
            source.SQLContent,
            options.join(", ")
        )
    } else {
        format!(
            "INSERT INTO {}.{} {}",
            ident(database),
            ident(shadow),
            source.SQLContent
        )
    })
}

struct RefreshSchema(Arc<dyn astersql_infoschema::infoschema::InfoSchema>);
impl astersql_executor::builder::InfoSchema for RefreshSchema {}
struct KvRefreshSnapshot(Box<dyn kv::Snapshot>);
impl astersql_executor::builder::Snapshot for KvRefreshSnapshot {
    fn set_option(
        &mut self,
        option: astersql_executor::builder::SnapshotOption,
    ) -> Result<(), astersql_executor::builder::BuildError> {
        use astersql_executor::builder::SnapshotOption;
        match option {
            SnapshotOption::TaskId(id) => self.0.SetOption(kv::TaskID, Some(Box::new(id))),
            SnapshotOption::ResourceGroupName(name) => self
                .0
                .SetOption(kv::ResourceGroupName, Some(Box::new(name))),
            SnapshotOption::ExplicitRequestSourceType(source) => self
                .0
                .SetOption(kv::ExplicitRequestSourceType, Some(Box::new(source))),
            option => {
                return Err(astersql_executor::builder::BuildError::new(format!(
                    "unsupported MV lookup snapshot option: {option:?}"
                )));
            }
        }
        Ok(())
    }
}

pub(super) fn initialize_session(
    session: &ConcreteSession,
    info: &astersql_meta_model::MaterializedViewInfo,
    caller: &ConcreteSession,
    best_effort: bool,
) -> SessionResult<SessionInitGuard> {
    let mut guard = SessionInitGuard {
        session: session.clone(),
        original: Vec::new(),
    };
    for name in [
        "sql_mode",
        "time_zone",
        "tidb_mview_enable",
        "div_precision_increment",
    ] {
        let original = rows(session, &format!("SELECT @@session.{name}"))?
            .first()
            .and_then(|row| row.first())
            .cloned()
            .ok_or_else(|| {
                SessionError::new(format!("cannot read refresh session variable {name}"))
            })?;
        guard.original.push((name, original));
    }
    let mode = astersql_parser_mysql::r#const::Str2SQLMode
        .iter()
        .filter(|(_, flag)| flag.0 != 0 && (info.DefinitionSQLMode.0 & flag.0) == flag.0)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",");
    sql(session, &format!("SET sql_mode={}", quoted(&mode)))?;
    let zone = if info.DefinitionTimeZone.name.is_empty() {
        let offset = info.DefinitionTimeZone.offset;
        format!(
            "{}{:02}:{:02}",
            if offset < 0 { "-" } else { "+" },
            offset.abs() / 3600,
            (offset.abs() % 3600) / 60
        )
    } else {
        info.DefinitionTimeZone.name.clone()
    };
    sql(session, &format!("SET time_zone={}", quoted(&zone)))?;
    sql(session, "SET tidb_mview_enable=ON")?;
    sql(
        session,
        &format!(
            "SET div_precision_increment={}",
            info.DefinitionDivPrecisionIncrement
        ),
    )?;
    for (source, target) in [
        ("tidb_mview_maintain_mem_quota", "tidb_mem_quota_query"),
        (
            "tidb_mview_maintain_isolation_read_engines",
            "tidb_isolation_read_engines",
        ),
        (
            "tidb_mview_maintain_import_threads",
            "tidb_mview_maintain_import_threads",
        ),
        (
            "tidb_mview_maintain_import_disk_quota",
            "tidb_mview_maintain_import_disk_quota",
        ),
        ("tidb_max_tiflash_threads", "tidb_max_tiflash_threads"),
        (
            "tidb_max_bytes_before_tiflash_external_join",
            "tidb_max_bytes_before_tiflash_external_join",
        ),
        (
            "tidb_max_bytes_before_tiflash_external_group_by",
            "tidb_max_bytes_before_tiflash_external_group_by",
        ),
        (
            "tidb_max_bytes_before_tiflash_external_sort",
            "tidb_max_bytes_before_tiflash_external_sort",
        ),
        (
            "tiflash_mem_quota_query_per_node",
            "tiflash_mem_quota_query_per_node",
        ),
        ("tiflash_query_spill_ratio", "tiflash_query_spill_ratio"),
        (
            "tiflash_fine_grained_shuffle_stream_count",
            "tiflash_fine_grained_shuffle_stream_count",
        ),
        (
            "tiflash_fine_grained_shuffle_batch_size",
            "tiflash_fine_grained_shuffle_batch_size",
        ),
    ] {
        if let Some(value) = caller.session_vars.GetSystemVar(source) {
            if let Some(original) = session.session_vars.GetSystemVar(target) {
                guard.original.push((target, original));
            }
            if let Err(error) = session
                .session_vars
                .SetHintSystemVarWithOldState(target, &value)
            {
                if !best_effort {
                    return Err(SessionError::new(error.to_string()));
                }
                session
                    .state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::warning(format!(
                        "refresh execution variable {target} could not be applied: {error}"
                    )));
            }
        }
    }
    Ok(guard)
}
pub(super) struct SessionInitGuard {
    session: ConcreteSession,
    original: Vec<(&'static str, String)>,
}
impl Drop for SessionInitGuard {
    fn drop(&mut self) {
        for (name, value) in self.original.iter().rev() {
            let _ = self
                .session
                .session_vars
                .SetHintSystemVarWithOldState(name, value);
            if let Err(error) = sql(&self.session, &format!("SET {name}={}", quoted(value))) {
                self.session
                    .state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::warning(format!(
                        "refresh session variable {name} could not be restored: {error}"
                    )));
            }
        }
    }
}
fn read_refresh_info(
    session: &ConcreteSession,
    id: i64,
    lock: bool,
) -> SessionResult<(Option<u64>, Option<i64>)> {
    let rows = rows(
        session,
        &format!(
            "SELECT LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={id}{}",
            if lock { " FOR UPDATE NOWAIT" } else { "" }
        ),
    )?;
    let row = rows.first().ok_or_else(|| {
        SessionError::new("refresh info row missing in mysql.tidb_mview_refresh_info")
    })?;
    let watermark = optional_u64(row.first())?;
    let end = row
        .get(1)
        .filter(|value| !matches!(value.to_ascii_lowercase().as_str(), "null" | "<nil>"))
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| watermark.map(|tso| (tso >> 18) as i64 / 1000));
    Ok((watermark, end))
}
fn persist_success(
    session: &ConcreteSession,
    id: i64,
    old: Option<u64>,
    read: u64,
    next: Option<Option<i64>>,
) -> SessionResult<()> {
    let next = next
        .map(|next| format!(",NEXT_REFRESH_UNIX_SECONDS={}", nullable(next)))
        .unwrap_or_default();
    sql(
        session,
        &format!(
            "UPDATE mysql.tidb_mview_refresh_info SET LAST_SUCCESS_READ_TSO={read},LAST_SUCCESS_REFRESH_END_UNIX_SECONDS={} {next} WHERE MVIEW_ID={id} AND LAST_SUCCESS_READ_TSO<=>{}",
            chrono::Utc::now().timestamp(),
            nullable(old)
        ),
    )?;
    if read_refresh_info(session, id, false)?.0 != Some(read) {
        return Err(SessionError::new(
            "inconsistent LAST_SUCCESS_READ_TSO after success update",
        ));
    }
    Ok(())
}
fn allocate_tso(session: &ConcreteSession) -> SessionResult<u64> {
    let mut tx = session
        .domain
        .storage()
        .with_storage(|store| store.Begin(&[]))
        .map_err(|e| session_error("allocate refresh job id", e))?;
    let ts = tx.StartTS();
    tx.Rollback()
        .map_err(|e| session_error("release refresh allocation transaction", e))?;
    if ts == 0 {
        return Err(SessionError::new("invalid refresh job id"));
    }
    Ok(ts)
}
fn insert_running(
    session: &ConcreteSession,
    job: u64,
    id: i64,
    database: &str,
    name: &str,
    method: &str,
    start: &str,
) -> SessionResult<()> {
    sql(
        session,
        &format!(
            "INSERT INTO mysql.tidb_mview_refresh_hist (REFRESH_JOB_ID,MVIEW_ID,MVIEW_SCHEMA,MVIEW_NAME,REFRESH_METHOD,REFRESH_START_TIME,REFRESH_STATUS,LAST_HEARTBEAT_TIME) VALUES ({job},{id},{},{},{},{start},'running',{start})",
            quoted(database),
            quoted(name),
            quoted(method)
        ),
    )
}
fn finalize_history(
    session: &ConcreteSession,
    job: u64,
    id: i64,
    status: &str,
    read: Option<u64>,
    commit: Option<u64>,
    _: &str,
    duration: Duration,
    schedule: Option<Duration>,
    scanned_rows: Option<u64>,
    reason: Option<&str>,
) -> SessionResult<()> {
    let statement = format!(
        "UPDATE mysql.tidb_mview_refresh_hist SET REFRESH_END_TIME=NOW(6),REFRESH_STATUS={},REFRESH_ROWS={},REFRESH_DURATION_SEC={},REFRESH_SCHEDULE_DURATION_SEC={},REFRESH_READ_TSO={},REFRESH_COMMIT_TSO={},REFRESH_FAILED_REASON={} WHERE REFRESH_JOB_ID={job} AND MVIEW_ID={id}",
        quoted(status),
        nullable(scanned_rows),
        astersql_executor::mview_task::format_duration_seconds(duration),
        schedule
            .map(|value| astersql_executor::mview_task::format_duration_seconds(value))
            .unwrap_or_else(|| "NULL".into()),
        nullable(read),
        nullable(commit),
        reason.map(quoted).unwrap_or_else(|| "NULL".into())
    );
    let run = || {
        fault("mockFinalizeRefreshHistError")?;
        sql(session, &statement)
    };
    match run() {
        Ok(()) => Ok(()),
        Err(first) => run().map_err(|second| {
            SessionError::new(format!("{second}; first finalize attempt failed: {first}"))
        }),
    }
}
fn insert_failed(
    session: &ConcreteSession,
    job: u64,
    id: i64,
    database: &str,
    name: &str,
    method: &str,
    start: &str,
    duration: Duration,
    read: Option<u64>,
    reason: &str,
) -> SessionResult<()> {
    fault("mockInsertRefreshHistFailedError")?;
    sql(
        session,
        &format!(
            "INSERT INTO mysql.tidb_mview_refresh_hist (REFRESH_JOB_ID,MVIEW_ID,MVIEW_SCHEMA,MVIEW_NAME,REFRESH_METHOD,REFRESH_START_TIME,REFRESH_END_TIME,REFRESH_STATUS,REFRESH_DURATION_SEC,REFRESH_READ_TSO,REFRESH_FAILED_REASON) VALUES ({job},{id},{},{},{},{start},NOW(6),'failed',{},{},{})",
            quoted(database),
            quoted(name),
            quoted(method),
            astersql_executor::mview_task::format_duration_seconds(duration),
            nullable(read),
            quoted(reason)
        ),
    )
}
fn check_log_integrity(
    history: &ConcreteSession,
    definition: &astersql_planner_core::MViewDefinition,
    last: u64,
) -> SessionResult<()> {
    if definition.mlog_table_id == 0 {
        return Err(SessionError::new(
            "materialized view log does not exist for base table",
        ));
    }
    let purged = rows(
        history,
        &format!(
            "SELECT LAST_PURGED_TSO FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
            definition.mlog_table_id
        ),
    )?;
    let purged_row = purged
        .first()
        .ok_or_else(|| SessionError::new("materialized view log purge info row is missing"))?;
    let purged = optional_u64(purged_row.first())?.unwrap_or(0);
    let cutoff = rows(
        history,
        &format!(
            "SELECT PURGE_CUTOFF_TSO FROM mysql.tidb_mlog_purge_hist WHERE MLOG_ID={} AND (PURGE_STATUS='running' OR PURGE_STATUS='orphaned' OR PURGE_ROWS>0) ORDER BY PURGE_JOB_ID DESC LIMIT 1",
            definition.mlog_table_id
        ),
    )?;
    let cutoff = cutoff
        .first()
        .map(|row| optional_u64(row.first()))
        .transpose()?
        .flatten()
        .unwrap_or(0);
    if purged.max(cutoff) > last {
        return Err(SessionError::new(
            "materialized view log may have been purged beyond LAST_SUCCESS_READ_TSO",
        ));
    }
    Ok(())
}

struct TaskMonitor {
    cancellation: Arc<std::sync::Mutex<Option<String>>>,
    stop: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl TaskMonitor {
    fn start(domain: Arc<Domain>, job: u64, id: i64, killer: Arc<SQLKiller>) -> Self {
        fn interval(name: &str, default: Duration) -> Duration {
            astersql_testkit_testfailpoint::eval_string(&format!(
                "github.com/pingcap/tidb/pkg/executor/{name}"
            ))
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_millis)
            .unwrap_or(default)
        }
        let poll = interval("mockMVTaskMonitorPollInterval", Duration::from_secs(5));
        let heartbeat = interval("mockMVTaskHistHeartbeatInterval", Duration::from_secs(600));
        let timeout = interval("mockMVTaskMonitorSQLTimeout", Duration::from_secs(5));
        let stop = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let notified = stop.clone();
        let cancellation = Arc::new(std::sync::Mutex::new(None));
        let requested = cancellation.clone();
        let worker = std::thread::spawn(move || {
            let history = ConcreteSession::new(domain);
            history.state.borrow_mut().in_restricted_sql = true;
            let mut next_heartbeat = Instant::now() + heartbeat;
            loop {
                if *notified
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                {
                    break;
                }
                if Instant::now() >= next_heartbeat {
                    let _deadline = super::typed_adapter_bridge::ExecutionDeadline::new(
                        history.sql_killer.clone(),
                        SystemTime::now(),
                        timeout.as_millis().min(u64::MAX as u128) as u64,
                    );
                    if let Err(error) = sql(
                        &history,
                        &format!(
                            "UPDATE mysql.tidb_mview_refresh_hist SET LAST_HEARTBEAT_TIME=NOW(6) WHERE REFRESH_JOB_ID={job} AND MVIEW_ID={id} AND REFRESH_STATUS='running'"
                        ),
                    ) {
                        BgLogger().log(
                            LogLevel::Warn,
                            "materialized view task heartbeat failed",
                            [
                                LogField::String("monitor".into(), "refresh".into()),
                                LogField::String("error".into(), error.to_string()),
                            ],
                        );
                    }
                    next_heartbeat = Instant::now() + heartbeat;
                }
                let cancel = {
                    let _deadline = super::typed_adapter_bridge::ExecutionDeadline::new(
                        history.sql_killer.clone(),
                        SystemTime::now(),
                        timeout.as_millis().min(u64::MAX as u128) as u64,
                    );
                    rows(
                        &history,
                        &format!(
                            "SELECT CANCEL_REQUEST_TIME,CANCEL_REQUESTED_BY FROM mysql.tidb_mview_refresh_hist WHERE REFRESH_JOB_ID={job} AND MVIEW_ID={id}"
                        ),
                    )
                };
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/executor/mvTaskMonitorPolled",
                );
                match cancel {
                    Ok(cancel) => {
                        if let Some(row) = cancel.first() {
                            if row.first().is_some_and(|value| {
                                !matches!(value.to_ascii_lowercase().as_str(), "null" | "<nil>")
                            }) {
                                let by = row
                                    .get(1)
                                    .filter(|value| {
                                        !matches!(
                                            value.to_ascii_lowercase().as_str(),
                                            "null" | "<nil>"
                                        )
                                    })
                                    .map(String::as_str)
                                    .unwrap_or("");
                                *requested
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    Some(if by.is_empty() {
                                        "cancelled manually".into()
                                    } else {
                                        format!("cancelled manually by {by}")
                                    });
                                killer.SendKillSignal(
                                    astersql_util_sqlkiller::sqlkiller::QueryInterrupted,
                                );
                                astersql_testkit_testfailpoint::inject(
                                    "github.com/pingcap/tidb/pkg/executor/mvTaskCancelWatcherRequested",
                                );
                                break;
                            }
                        }
                    }
                    Err(error) => BgLogger().log(
                        LogLevel::Warn,
                        "materialized view task monitor cancel poll failed",
                        [
                            LogField::String("monitor".into(), "refresh".into()),
                            LogField::String("error".into(), error.to_string()),
                        ],
                    ),
                }
                let guard = notified
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (guard, _) = notified
                    .1
                    .wait_timeout_while(guard, poll, |stop| !*stop)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *guard {
                    break;
                }
            }
        });
        Self {
            cancellation,
            stop,
            worker: Some(worker),
        }
    }
    fn check_cancelled(&self) -> SessionResult<()> {
        if self
            .cancellation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            Err(SessionError::new(
                "materialized view task canceled manually",
            ))
        } else {
            Ok(())
        }
    }
    fn stop(&mut self) {
        let (lock, wake) = &*self.stop;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for TaskMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ShadowGuard {
    session: ConcreteSession,
    database: String,
    source: astersql_meta_model::TableInfo,
    name: String,
    id: Cell<i64>,
    armed: Cell<bool>,
    read_ts: Cell<u64>,
    cleanup_duration: Cell<Duration>,
}
impl ShadowGuard {
    fn new(
        session: ConcreteSession,
        database: String,
        source: astersql_meta_model::TableInfo,
    ) -> Self {
        let name = format!(
            "__mv_shadow_{}_{}",
            source.ID,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        Self {
            session,
            database,
            source,
            name,
            id: Cell::new(0),
            armed: Cell::new(false),
            read_ts: Cell::new(0),
            cleanup_duration: Cell::new(Duration::ZERO),
        }
    }
    fn create(&mut self) -> SessionResult<()> {
        use astersql_ddl_jobsubmit::SessionPool;
        let pool = super::system_session::SystemSessionPool::new(self.session.domain.clone());
        let mut allocator =
            SessionPool::get(pool.as_ref()).map_err(|e| SessionError::new(e.to_string()))?;
        let allocated = (|| {
            allocator.begin()?;
            let version = allocator.current_version()?;
            allocator.lock_global_id_key(version)?;
            let id = allocator.generate_global_ids(1)?[0];
            allocator.commit()?;
            Ok::<_, astersql_ddl_jobsubmit::Error>(id)
        })();
        if allocated.is_err() {
            allocator.rollback();
        }
        SessionPool::put(pool.as_ref(), allocator);
        let id = allocated.map_err(|e| SessionError::new(e.to_string()))?;
        let mut shadow = self.source.clone();
        shadow.ID = id;
        shadow.Revision = 0;
        shadow.Name = ast::NewCIStr(&self.name);
        shadow.MaterializedView = None;
        shadow.MaterializedViewShadow = Some(astersql_meta_model::MaterializedViewShadowInfo {
            SourceMViewID: self.source.ID,
        });
        shadow.State = astersql_meta_model::SchemaState::None;
        let mut job = astersql_meta_model::group_3::Job {
            tp: 93,
            schema_id: self.source.DBID,
            table_id: id,
            schema_name: self.database.clone(),
            table_name: self.name.clone(),
            raw_args: serde_json::to_vec(&serde_json::json!({"table_info":shadow,"fk_check":true}))
                .map_err(|e| session_error("encode refresh shadow", e))?,
            ..Default::default()
        };
        super::normal_ddl_submit::submit_and_wait(
            &pool,
            &astersql_owner::manager::Context::new(),
            None,
            &mut job,
        )
        .map_err(SessionError::new)?;
        self.id.set(id);
        self.armed.set(true);
        self.session
            .domain
            .refresh_materialized_view_metadata()
            .map_err(|error| session_error("publish shadow create metadata", error))?;
        Ok(())
    }
    fn cleanup(&mut self) -> SessionResult<()> {
        if !self.armed.replace(false) {
            return Ok(());
        }
        let start = Instant::now();
        let result: SessionResult<()> = (|| {
            fault("mockRefreshMaterializedViewShadowCleanupError")?;
            drop_shadow_table(&self.session, &self.database, &self.name)?;
            Ok(())
        })();
        self.cleanup_duration.set(start.elapsed());
        if let Err(error) = &result {
            BgLogger().log(
                LogLevel::Warn,
                "failed to cleanup shadow table after out-of-place complete refresh error",
                [
                    LogField::String("schema".into(), self.database.clone()),
                    LogField::String("mview".into(), self.source.Name.O.clone()),
                    LogField::String("shadowTable".into(), self.name.clone()),
                    LogField::String("error".into(), error.to_string()),
                ],
            );
        }
        result
    }
}
impl Drop for ShadowGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

impl ConcreteSession {
    pub(super) fn execute_cancel_refresh_job(&self, job: i64) -> SessionResult<()> {
        if job <= 0 {
            return Err(SessionError::new(format!(
                "cannot cancel materialized view refresh job {job}"
            )));
        }
        let history = ConcreteSession::new(self.domain.clone());
        history.state.borrow_mut().in_restricted_sql = true;
        if self.login_user.is_some()
            && self
                .domain
                .info_schema()
                .TableByName(
                    &astersql_infoschema::CiString::new("mysql"),
                    &astersql_infoschema::CiString::new("tidb_mview_refresh_hist"),
                )
                .is_err()
        {
            return Err(SessionError::new(
                "refresh materialized view: required system table mysql.tidb_mview_refresh_hist does not exist",
            ));
        }
        let target = rows(
            &history,
            &format!(
                "SELECT MVIEW_ID,REFRESH_STATUS,CANCEL_REQUEST_TIME FROM mysql.tidb_mview_refresh_hist WHERE REFRESH_JOB_ID={job}"
            ),
        )?;
        let target = target
            .first()
            .filter(|row| row.get(1).is_some_and(|status| status == "running"))
            .ok_or_else(|| {
                SessionError::new(format!("cannot cancel materialized view refresh job {job}"))
            })?;
        if self.login_user.is_some() {
            let id = target[0]
                .parse::<i64>()
                .map_err(|e| session_error("read refresh cancellation target", e))?;
            let schema = self.domain.info_schema();
            let table = schema.TableByID(id).ok_or_else(|| {
                SessionError::new(format!("refresh materialized view: cannot resolve target materialized view {id} for cancel job {job}"))
            })?;
            let meta = table
                .ModelMeta()
                .map_err(|e| session_error("read cancellation target metadata", e))?;
            let database = astersql_infoschema::SchemaByTable(schema.as_ref(), table.Meta())
                .ok_or_else(|| {
                    SessionError::new(format!("refresh materialized view: cannot resolve schema for materialized view {id}"))
                })?;
            self.check_mlog_operate_privilege(&database.name.lower, &meta.Name.L)?;
        }
        let requester = self
            .login_user
            .as_deref()
            .map(|user| {
                quoted(&format!(
                    "'{user}'@'{}'",
                    self.authenticated_host
                        .as_deref()
                        .or(self.login_host.as_deref())
                        .unwrap_or("")
                ))
            })
            .unwrap_or_else(|| "NULL".into());
        sql(
            &history,
            &format!(
                "UPDATE mysql.tidb_mview_refresh_hist SET CANCEL_REQUEST_TIME=NOW(6),CANCEL_REQUESTED_BY={requester} WHERE REFRESH_JOB_ID={job} AND REFRESH_STATUS='running' AND CANCEL_REQUEST_TIME IS NULL"
            ),
        )?;
        if history.protocol_state().affected_rows == 0 {
            return Err(SessionError::new(format!(
                "cannot cancel materialized view refresh job {job}"
            )));
        }
        Ok(())
    }
}

impl ConcreteSession {
    fn check_refresh_base_select(
        &self,
        info: &astersql_meta_model::MaterializedViewInfo,
    ) -> SessionResult<()> {
        let Some(user) = self.login_user.as_deref() else {
            return Ok(());
        };
        let host = self
            .authenticated_host
            .as_deref()
            .or(self.login_host.as_deref())
            .unwrap_or("%");
        for id in &info.BaseTableIDs {
            if let Some(table) = self.domain.info_schema().TableByID(*id) {
                let schema = astersql_infoschema::SchemaByTable(
                    self.domain.info_schema().as_ref(),
                    table.Meta(),
                );
                let table = table
                    .ModelMeta()
                    .map_err(|e| session_error("resolve refresh privilege target", e))?;
                if let Some(schema) = schema {
                    if !runtime_privilege_handle(&self.domain)
                        .Get()
                        .RequestVerification(
                            &self.active_roles.borrow(),
                            user,
                            host,
                            &schema.name.lower,
                            &table.Name.L,
                            "",
                            astersql_privilege_privileges::SelectPriv,
                        )
                    {
                        return Err(SessionError::new(format!(
                            "SELECT command denied to user '{user}'@'{host}' for table '{}'",
                            table.Name.O
                        )));
                    }
                }
            }
        }
        Ok(())
    }
    fn evaluate_refresh_target_ts(&self, expression: &ast::ExprNode) -> SessionResult<u64> {
        self.evaluate_stale_read_ts(expression)
    }
}

impl ConcreteSession {
    pub(super) fn execute_refresh_sql(
        &self,
        statement: &ast::RefreshMaterializedViewStmt,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        use ast::RefreshMaterializedViewObserveType as Observe;
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return Err(SessionError::new(
                "materialized view refresh requires the MySQL dialect",
            ));
        }
        if self.state.borrow().transaction.is_some() {
            return Err(SessionError::new(
                "cannot run REFRESH MATERIALIZED VIEW in explicit transaction",
            ));
        }
        let mode = statement.mode().map_err(SessionError::new)?;
        if statement.WithAsyncMode {
            return Err(SessionError::new(
                "refresh materialized view: WITH ASYNC MODE is not supported yet",
            ));
        }
        if statement.AsOf.is_some() && mode != Mode::Fast {
            return Err(SessionError::new(
                "refresh materialized view: AS OF TIMESTAMP is only supported for FAST refresh",
            ));
        }
        let target = statement
            .ViewName
            .as_ref()
            .ok_or_else(|| SessionError::new("refresh materialized view: missing view name"))?;
        let database = if target.Schema.L.is_empty() {
            self.current_database()
        } else {
            target.Schema.O.clone()
        };
        self.check_mlog_operate_privilege(&database, &target.Name.L)?;
        if statement.ObserveType != Observe::None {
            self.check_refresh_show_privilege(&database, &target.Name.L)?;
        }
        let mut plain = statement.clone();
        plain.ObserveType = Observe::None;
        let mut executor = RefreshMaterializedViewExec::new(self.clone(), plain.clone(), false);
        match statement.ObserveType {
            Observe::None => {
                executor.next(None)?;
                Ok(None)
            }
            Observe::DryRun => {
                let plans = self.refresh_dry_plans(&plain, &database)?;
                let mut output = Vec::new();
                for step in new_step_set(mode) {
                    output.push(vec![format!("[{} {}]", step.id, step.name)]);
                    if let Some(rows) = plans.get(step.name) {
                        output.extend(format_observe_plan(rows).into_iter().map(|line| vec![line]));
                    }
                }
                Ok(Some(ConcreteRecordSet::new(
                    vec!["Refresh Plan".into()],
                    output,
                )))
            }
            Observe::Profile => {
                let mut collector = RefreshObserveCollector::default();
                executor.next(Some(&mut collector))?;
                let mut output = Vec::new();
                for step in new_step_set(mode) {
                    let runtime = collector.steps.get(&step.id);
                    let suffix = runtime
                        .and_then(|r| r.start.zip(r.end).map(|(start, end)| (r, start, end)))
                        .map(|(r, start, end)| {
                            format!(
                                " status:{}, time:{:?}",
                                if r.error.is_some() {
                                    "failed"
                                } else {
                                    "success"
                                },
                                end.duration_since(start).unwrap_or_default()
                            )
                        })
                        .unwrap_or_default();
                    output.push(vec![format!("[{} {}]{suffix}", step.id, step.name)]);
                    if let Some(runtime) = runtime {
                        output.extend(
                            format_observe_profile_plan(&runtime.plans)
                                .into_iter()
                                .map(|line| vec![line]),
                        );
                    }
                }
                Ok(Some(ConcreteRecordSet::new(
                    vec!["Refresh Profile".into()],
                    output,
                )))
            }
        }
    }
    fn refresh_write_plan(&self, operator: &str, source: &str) -> SessionResult<Vec<Vec<String>>> {
        let source = astersql_parser::New()
            .ParseOneStmt(source, "", "")
            .map_err(|e| session_error("parse refresh write source", e))?;
        let source = source
            .into_any()
            .downcast::<ast::SelectStmt>()
            .map_err(|_| SessionError::new("refresh write source is not SELECT"))?;
        let read_ts = self
            .domain
            .storage()
            .with_storage(|store| store.CurrentVersion("global"))
            .map_err(|e| session_error("read refresh write plan TSO", e))?
            .Ver;
        let _child = self
            .compile_mview_source(*source, read_ts)
            .map_err(|e| SessionError::new(e.to_string()))?;
        // The runtime owns the write sink; compile its actual relational input
        // instead of using the general legacy EXPLAIN placeholder.
        let mut plan = vec![vec![format!("{operator}_0")]];
        plan.extend(self.mview_plan_rows.borrow().clone());
        Ok(plan)
    }
    fn check_refresh_show_privilege(&self, database: &str, view: &str) -> SessionResult<()> {
        let Some(user) = self.login_user.as_deref() else {
            return Ok(());
        };
        let host = self
            .authenticated_host
            .as_deref()
            .or(self.login_host.as_deref())
            .unwrap_or("%");
        if !runtime_privilege_handle(&self.domain)
            .Get()
            .RequestVerification(
                &self.active_roles.borrow(),
                user,
                host,
                database,
                view,
                "",
                astersql_privilege_privileges::ShowViewPriv,
            )
        {
            return Err(SessionError::new(format!(
                "SHOW VIEW command denied to user '{user}'@'{host}' for table '{view}'"
            )));
        }
        Ok(())
    }
    fn refresh_dry_plans(
        &self,
        statement: &ast::RefreshMaterializedViewStmt,
        database: &str,
    ) -> SessionResult<HashMap<&'static str, Vec<Vec<String>>>> {
        let mode = statement.mode().map_err(SessionError::new)?;
        let target = statement.ViewName.as_ref().unwrap();
        let table = self
            .resolve_runtime_table(database, &target.Name.L)
            .ok_or_else(|| SessionError::new("refresh materialized view target does not exist"))?;
        let info = table
            .MaterializedView
            .as_ref()
            .ok_or_else(|| SessionError::new("target is not a materialized view"))?;
        if !info.GetInitBuildState().IsReady() {
            return Err(SessionError::new(
                info.GetInitBuildState()
                    .AccessErrorMessage(&format!("{database}.{}", table.Name.O)),
            ));
        }
        self.check_refresh_base_select(info)?;
        let maintenance = ConcreteSession::new(self.domain.clone());
        maintenance.state.borrow_mut().in_restricted_sql = true;
        sql(&maintenance, &format!("USE {}", ident(database)))?;
        let _settings = initialize_session(&maintenance, info, self, false)?;
        let mut plans = HashMap::new();
        let target_name = format!("{}.{}", ident(database), ident(&table.Name.O));
        match mode {
            Mode::CompleteInPlace | Mode::CompleteOutOfPlace => {
                if mode == Mode::CompleteInPlace {
                    plans.insert(
                        "DATA_CHANGE_COMPLETE_DELETE",
                        maintenance.refresh_write_plan(
                            "Delete",
                            &format!("SELECT * FROM {target_name}"),
                        )?,
                    );
                }
                plans.insert(
                    if mode == Mode::CompleteInPlace {
                        "DATA_CHANGE_COMPLETE_INSERT"
                    } else {
                        "OUT_OF_PLACE_LOAD_SHADOW"
                    },
                    maintenance.refresh_write_plan(
                        if mode == Mode::CompleteOutOfPlace
                            && maintenance
                                .domain
                                .storage()
                                .with_storage(|store| store.Name())
                                == "TiKV"
                        {
                            "ImportInto"
                        } else {
                            "Insert"
                        },
                        &info.SQLContent,
                    )?,
                );
            }
            Mode::Fast | Mode::CompleteDeltaApply => {
                let (definition, select, _) = definition(&maintenance, database, &table)?;
                let read_ts = self
                    .domain
                    .storage()
                    .with_storage(|store| store.CurrentVersion("global"))
                    .map_err(|e| session_error("read refresh plan TSO", e))?
                    .Ver;
                let (source, operator) = if mode == Mode::Fast {
                    let history = ConcreteSession::new(self.domain.clone());
                    let history_rows = rows(
                        &history,
                        &format!(
                            "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                            table.ID
                        ),
                    )?;
                    let watermark = history_rows.first().ok_or_else(||SessionError::new("refresh materialized view: refresh info row missing in mysql.tidb_mview_refresh_info"))?;
                    let watermark = optional_u64(watermark.first())?.ok_or_else(|| {
                        SessionError::new(
                            "refresh materialized view fast: LAST_SUCCESS_READ_TSO is NULL",
                        )
                    })?;
                    let target_ts = statement
                        .AsOf
                        .as_ref()
                        .map(|a| self.evaluate_refresh_target_ts(&a.TsExpr))
                        .transpose()?
                        .unwrap_or(0);
                    if target_ts > 0 && target_ts == watermark {
                        return Ok(plans);
                    }
                    check_log_integrity(&history, &definition, watermark)?;
                    if target_ts > 0 && target_ts < watermark {
                        return Err(SessionError::new(format!(
                            "refresh materialized view fast as of timestamp: target tso {target_ts} is older than LAST_SUCCESS_READ_TSO {watermark}"
                        )));
                    }
                    let log = self
                        .domain
                        .info_schema()
                        .TableByID(definition.mlog_table_id)
                        .ok_or_else(|| SessionError::new("materialized view log does not exist"))?
                        .ModelMeta()
                        .map_err(|e| session_error("read MLog plan metadata", e))?;
                    let (source, _) =
                        astersql_planner_core::mview_refresh_source::BuildFastRefreshSource(
                            &definition,
                            &table.Name.O,
                            &log.Name.O,
                            &select,
                            MViewBuildOptions {
                                from_ts: watermark,
                                to_ts: target_ts,
                            },
                        )
                        .map_err(SessionError::new)?;
                    (source, "MViewDeltaMerge")
                } else {
                    let (source, _) =
                        astersql_planner_core::mview_refresh_source::BuildCompleteRefreshSource(
                            &definition,
                            &table.Name.O,
                            *select,
                        )
                        .map_err(SessionError::new)?;
                    (source, "MViewCompleteDeltaApply")
                };
                // Building this executor compiles its physical source without opening it:
                // no history, watermark, shadow or table mutation is performed.
                let _child = maintenance
                    .compile_mview_source(source, read_ts)
                    .map_err(|e| SessionError::new(e.to_string()))?;
                let mut plan = vec![vec![format!("{operator}_0")]];
                plan.extend(maintenance.mview_plan_rows.borrow().clone());
                plans.insert(
                    if mode == Mode::Fast {
                        "DATA_CHANGE_FAST_MERGE"
                    } else {
                        "DATA_CHANGE_COMPLETE_DELTA_APPLY"
                    },
                    plan,
                );
            }
        }
        Ok(plans)
    }
}
#[derive(Default)]
struct RefreshStepRuntime {
    start: Option<SystemTime>,
    end: Option<SystemTime>,
    error: Option<String>,
    plans: Vec<Vec<String>>,
}
#[derive(Default)]
struct RefreshObserveCollector {
    steps: HashMap<String, RefreshStepRuntime>,
}
impl StepObserver for RefreshObserveCollector {
    fn on_step_start(&mut self, step: &ObserveStep, at: SystemTime) {
        self.steps.entry(step.id.clone()).or_default().start = Some(at);
    }
    fn on_step_end(&mut self, step: &ObserveStep, at: SystemTime, error: Option<&str>) {
        let r = self.steps.entry(step.id.clone()).or_default();
        r.end = Some(at);
        r.error = error.map(str::to_owned);
    }
    fn on_step_plan_rows(&mut self, step: &ObserveStep, rows: Vec<Vec<String>>) {
        self.steps.entry(step.id.clone()).or_default().plans = rows;
    }
}
fn format_observe_plan(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter()
        .filter(|row| !row.is_empty())
        .map(|row| {
            static PLAN_SUFFIX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            let id = PLAN_SUFFIX
                .get_or_init(|| {
                    regex::Regex::new(r"_(\d+)\b").expect("constant plan suffix pattern")
                })
                .replace_all(&row[0], "")
                .into_owned();
            let mut fields = vec![id];
            if let Some(value) = row.get(1).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                fields.push(format!("estRows:{value}"));
            }
            if let Some(value) = row.get(2).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                fields.push(format!("task:{value}"));
            }
            for value in row
                .iter()
                .skip(3)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case("N/A"))
            {
                fields.push(value.into());
            }
            format!("  {}", fields.join(" | "))
        })
        .collect()
}

fn new_refresh_runtime_stats() -> TypedRuntimeStats {
    Arc::new(std::sync::Mutex::new(
        astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None),
    ))
}
fn analyze_refresh_plan_rows(
    rows: Vec<Vec<String>>,
    stats: &TypedRuntimeStats,
) -> Vec<Vec<String>> {
    let stats = stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rows.into_iter()
        .map(|row| {
            let id = row
                .first()
                .and_then(|id| id.rsplit_once('_'))
                .and_then(|(_, id)| id.parse::<i32>().ok());
            let observed = id.and_then(|id| stats.GetRootStatsIfExists(id));
            let field = |index| row.get(index).cloned().unwrap_or_default();
            vec![
                field(0),
                field(1),
                observed
                    .map(|s| s.GetActRows().to_string())
                    .unwrap_or_default(),
                "root".into(),
                String::new(),
                id.filter(|_| observed.is_some())
                    .map(|id| stats.GetRootStatsStringShared(id))
                    .unwrap_or_default(),
                field(3),
                "N/A".into(),
                "N/A".into(),
            ]
        })
        .collect()
}

fn format_observe_profile_plan(rows: &[Vec<String>]) -> Vec<String> {
    rows.iter()
        .flat_map(|row| {
            if row.len() != 9 {
                return format_observe_plan(std::slice::from_ref(row));
            }
            let mut fields = format_observe_plan(&[vec![row[0].clone(), row[1].clone()]])
                .pop()
                .unwrap_or_default();
            if !row[2].trim().is_empty() {
                fields.push_str(&format!(" | actRows:{}", row[2].trim()));
            }
            if !row[3].trim().is_empty() {
                fields.push_str(&format!(" | task:{}", row[3].trim()));
            }
            for value in row
                .iter()
                .skip(4)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("N/A"))
            {
                fields.push_str(" | ");
                fields.push_str(value);
            }
            vec![fields]
        })
        .collect()
}

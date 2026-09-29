// Copyright 2026 AsterSQL.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use astersql_errors as errors;
use astersql_executor::adapter::*;
use astersql_executor::typed_point_get::PointLockRuntime;
use astersql_kv::Getter;
use astersql_plugin as plugin;
use astersql_util_chunk as chunk;

use super::SessionBoundAdapterOwner;

struct SessionPessimisticTransaction {
    session: Rc<super::ConcreteSession>,
    writes_before: std::collections::HashSet<super::RuntimeRowLockKey>,
}

struct SessionPointLockRuntime {
    session: Rc<super::ConcreteSession>,
    read_ts: u64,
    read_committed: bool,
}

struct AdapterSummaryLazyInfo(StatementSummary);

impl astersql_util_stmtsummary::StmtExecLazyInfo for AdapterSummaryLazyInfo {
    fn GetOriginalSQL(&self) -> String {
        self.0.original_sql.clone()
    }
    fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
        (self.0.encoded_plan.clone(), String::new(), None)
    }
    fn GetBinaryPlan(&self) -> String {
        self.0.binary_plan.clone()
    }
    fn GetPlanDigest(&self) -> String {
        self.0.plan_digest.clone()
    }
    fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (String::new(), String::new())
    }
}

impl SessionBoundAdapterOwner {
    fn begin_statement_mutation_stage(&self) -> AdapterResult {
        let mut state = self.session.state.borrow_mut();
        let baseline = (
            state.transaction_write_keys.clone(),
            state.txn_mem_buffer_keys,
            state.txn_mem_buffer_bytes,
            state.transaction_conflict_context.clone(),
            state.last_dml_report.clone(),
            state.pending_fk_delete_cascades.clone(),
        );
        let handle = state
            .transaction
            .as_mut()
            .ok_or_else(|| errors::New("canonical pessimistic transaction is missing"))?
            .StageStatement()?;
        state.adapter_dml_statement_staged = true;
        *self.statement_mutation_stage.borrow_mut() =
            Some(super::typed_adapter_bridge::StatementMutationStage {
                handle,
                write_keys_before: baseline.0,
                buffer_keys_before: baseline.1,
                buffer_bytes_before: baseline.2,
                conflict_before: baseline.3,
                dml_report_before: baseline.4,
                pending_cascades_before: baseline.5,
            });
        Ok(())
    }

    fn finish_statement_mutation_stage(&self, success: bool) -> AdapterResult {
        let Some(stage) = self.statement_mutation_stage.borrow_mut().take() else {
            return Ok(());
        };
        let mut state = self.session.state.borrow_mut();
        if let Some(transaction) = state.transaction.as_mut() {
            if success {
                transaction.ReleaseStatement(stage.handle)?;
            } else {
                transaction.CleanupStatement(stage.handle)?;
            }
        }
        if !success {
            state.transaction_write_keys = stage.write_keys_before;
            state.txn_mem_buffer_keys = stage.buffer_keys_before;
            state.txn_mem_buffer_bytes = stage.buffer_bytes_before;
            state.transaction_conflict_context = stage.conflict_before;
            state.last_dml_report = stage.dml_report_before;
            state.pending_fk_delete_cascades = stage.pending_cascades_before;
        }
        state.adapter_dml_statement_staged = false;
        Ok(())
    }
}

impl PointLockRuntime for SessionPointLockRuntime {
    fn IsReadCommitted(&self) -> bool {
        self.read_committed
    }
    fn LocalValue(&self, key: astersql_kv::Key) -> AdapterResult<Option<Option<Vec<u8>>>> {
        let state = self.session.state.borrow();
        let runtime_key = super::RuntimeRowLockKey {
            domain_id: Arc::as_ptr(&self.session.domain) as usize,
            key: key.0.clone(),
        };
        if !state.transaction_write_keys.contains(&runtime_key) {
            return Ok(None);
        }
        let transaction = state
            .transaction
            .as_ref()
            .ok_or_else(|| errors::New("local PointGet mutation has no canonical transaction"))?;
        match transaction.Get(&astersql_kv::Context::default(), key, &[]) {
            Ok(value) => Ok(Some(Some(value.Value))),
            Err(error) if astersql_kv::ErrNotExist.Equal(Some(&error)) => Ok(Some(None)),
            Err(error) => Err(error),
        }
    }
    fn LockKey(
        &self,
        key: astersql_kv::Key,
        only_if_exists: bool,
        wait_ms: i64,
    ) -> AdapterResult<Option<Vec<u8>>> {
        if !self.session.TransactionIsPessimistic() {
            return Err(errors::New(
                "locking PointGet requires an active canonical pessimistic transaction",
            ));
        }
        let local = self.LocalValue(key.clone())?;
        if only_if_exists && local == Some(None) {
            return Ok(None);
        }
        let current = self
            .session
            .domain
            .storage()
            .with_storage(|storage| storage.CurrentVersion("global"))?;
        let latest = self.session.domain.storage().with_storage(|storage| {
            storage
                .GetSnapshot(current)
                .Get(&astersql_kv::Context::default(), key.clone(), &[])
        });
        let prior = match latest {
            Ok(value) => {
                if local.is_none() && !self.read_committed && value.CommitTs > self.read_ts {
                    return Err(astersql_kv::ErrWriteConflict.FastGenByArgs(&[]));
                }
                Some(value.Value)
            }
            Err(error) if astersql_kv::ErrNotExist.Equal(Some(&error)) => None,
            Err(error) => return Err(error),
        };
        if only_if_exists && prior.is_none() && local.is_none() {
            return Ok(None);
        }
        let runtime_key = super::RuntimeRowLockKey {
            domain_id: Arc::as_ptr(&self.session.domain) as usize,
            key: key.0.clone(),
        };
        let already_held = self
            .session
            .state
            .borrow()
            .held_row_locks
            .contains(&runtime_key);
        self.session
            .acquire_point_row_lock(runtime_key.clone(), wait_ms)
            .map_err(|error| errors::New(error.to_string()))?;
        let outcome = (|| -> AdapterResult<Option<Vec<u8>>> {
            if let Some(value) = local {
                return Ok(if self.read_committed { value } else { None });
            }
            let refreshed = self
                .session
                .domain
                .storage()
                .with_storage(|storage| storage.CurrentVersion("global"))?;
            let value = self.session.domain.storage().with_storage(|storage| {
                storage.GetSnapshot(refreshed).Get(
                    &astersql_kv::Context::default(),
                    key.clone(),
                    &[],
                )
            });
            match value {
                Ok(value) => {
                    if !self.read_committed && value.CommitTs > self.read_ts {
                        return Err(astersql_kv::ErrWriteConflict.FastGenByArgs(&[]));
                    }
                    Ok(self.read_committed.then_some(value.Value))
                }
                Err(error) if astersql_kv::ErrNotExist.Equal(Some(&error)) => {
                    if !self.read_committed {
                        let historical = self.session.domain.storage().with_storage(|storage| {
                            storage
                                .GetSnapshot(astersql_kv::Version { Ver: self.read_ts })
                                .Get(&astersql_kv::Context::default(), key, &[])
                        });
                        match historical {
                            Ok(_) => return Err(astersql_kv::ErrWriteConflict.FastGenByArgs(&[])),
                            Err(error) if astersql_kv::ErrNotExist.Equal(Some(&error)) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    Ok(None)
                }
                Err(error) => Err(error),
            }
        })();
        if !already_held
            && (outcome.is_err() || (only_if_exists && outcome.as_ref().is_ok_and(Option::is_none)))
        {
            let acquired = std::collections::HashSet::from([runtime_key.clone()]);
            super::release_runtime_row_locks(self.session.row_lock_owner, Some(&acquired));
            self.session
                .state
                .borrow_mut()
                .held_row_locks
                .remove(&runtime_key);
        }
        outcome
    }
}

impl pessimisticTxn for SessionPessimisticTransaction {
    fn KeysNeedToLock(&mut self) -> AdapterResult<Vec<Key>> {
        let state = self.session.state.borrow();
        let mut keys = state
            .transaction_write_keys
            .difference(&self.writes_before)
            .map(|key| key.key.clone())
            .collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }
    fn IsValid(&self) -> bool {
        self.session
            .state
            .borrow()
            .transaction
            .as_ref()
            .is_some_and(|transaction| transaction.Valid())
    }
    fn IsKeyWritten(&self, key: &[u8]) -> AdapterResult<bool> {
        let runtime_key = super::RuntimeRowLockKey {
            domain_id: Arc::as_ptr(&self.session.domain) as usize,
            key: key.to_vec(),
        };
        Ok(self
            .session
            .state
            .borrow()
            .transaction_write_keys
            .contains(&runtime_key))
    }
}

impl AdapterRuntime for SessionBoundAdapterOwner {
    fn BuildExecutor(
        &self,
        plan: &PlanInfo,
        telemetry: Option<&TelemetryInfo>,
    ) -> AdapterResult<Box<dyn ExecExecutor>> {
        if plan.kind == PlanKind::Analyze {
            let sql =
                self.analyze_sql.borrow().clone().ok_or_else(|| {
                    errors::New("canonical ANALYZE was not bound to session runtime")
                })?;
            return Ok(Box::new(
                super::typed_analyze_executor::SessionTypedAnalyzeExecutor::new(
                    Rc::clone(&self.session),
                    sql,
                ),
            ));
        }
        if plan.IsDML() {
            let sql = self
                .dml_sql
                .borrow()
                .clone()
                .ok_or_else(|| errors::New("canonical DML was not bound to session runtime"))?;
            return Ok(Box::new(
                super::typed_dml_executor::SessionTypedDMLExecutor::new(
                    Rc::clone(&self.session),
                    sql,
                    plan.kind,
                ),
            ));
        }
        let spec = self
            .scan
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| errors::New("typed scan was not bound to session runtime"))?;
        if plan.kind != PlanKind::Query {
            return Err(errors::New(
                "session-bound typed scan runtime requires a query plan",
            ));
        }
        if let Some(physical) = self.physical_scan.borrow().as_ref() {
            let executor = if physical.leaf_ranges.len() > 1 {
                self.session.OpenTypedPhysicalPlanWithBindings(
                    physical.plan.as_plan(),
                    &physical.leaf_ranges,
                    physical.version,
                    physical.initial_capacity,
                    physical.maximum_chunk_size,
                )
            } else {
                self.OpenTypedPhysicalTableScan(
                    physical.plan.as_plan(),
                    physical.ranges.clone(),
                    physical.version,
                    physical.initial_capacity,
                    physical.maximum_chunk_size,
                )
            }
            .map_err(|error| errors::New(error.to_string()))?;
            return Ok(executor);
        }
        Ok(self.OpenTypedKVSnapshotScan(
            spec.table_id,
            spec.pk_is_handle,
            spec.descending,
            spec.columns,
            spec.ranges,
            spec.version,
            spec.initial_capacity,
            spec.maximum_chunk_size,
        ))
    }
    fn BuildExecutorForSelectLock(
        &self,
        plan: &PlanInfo,
        _telemetry: Option<&TelemetryInfo>,
    ) -> AdapterResult<Box<dyn ExecExecutor>> {
        if plan.kind != PlanKind::Query || !self.SupportsSelectForUpdate() {
            return Err(errors::New(
                "canonical typed SelectLock plan is not supported",
            ));
        }
        let physical = self.physical_scan.borrow();
        let binding = physical
            .as_ref()
            .ok_or_else(|| errors::New("typed SelectLock physical plan was not bound"))?;
        let executor = self
            .OpenTypedPhysicalSelectLockPlan(
                binding.plan.as_plan(),
                binding.ranges.clone(),
                binding.version,
                binding.initial_capacity,
                binding.maximum_chunk_size,
            )
            .map_err(|error| errors::New(error.to_string()))?;
        Ok(executor)
    }
    fn BuildPointGetExecutor(
        &self,
        plan: &PlanInfo,
        start_ts: u64,
        prepared_key: Option<&str>,
    ) -> AdapterResult<Box<dyn ExecExecutor>> {
        if plan.kind != PlanKind::PointGet {
            return Err(errors::New("point get requires a PointGet plan"));
        }
        let physical = self.physical_scan.borrow();
        let binding = physical
            .as_ref()
            .ok_or_else(|| errors::New("PointGet physical plan was not bound"))?;
        if !binding
            .plan
            .as_plan()
            .as_any()
            .is::<astersql_planner_core_operator_physicalop::PointGetPlan>()
            || binding.version.Ver != start_ts
        {
            return Err(errors::New(
                "PointGet plan/read TS do not match the canonical binding",
            ));
        }
        let point = binding
            .plan
            .as_plan()
            .as_any()
            .downcast_ref::<astersql_planner_core_operator_physicalop::PointGetPlan>()
            .expect("checked PointGet physical type");
        let (weak_consistency, redact_mode) = self.session.WithSessionVars(|vars| {
            (
                vars.StmtCtx.WeakConsistency,
                vars.GetSystemVar("tidb_redact_log")
                    .unwrap_or_else(|| "OFF".into())
                    .to_ascii_uppercase(),
            )
        });
        if start_ts == u64::MAX
            && !point.Lock
            && self.session.state.borrow().transaction.is_none()
            && point.PartitionIdx.is_none()
            && !point.IsTableDual
        {
            if let Some(key) = prepared_key {
                let cached = { self.point_cache.borrow().get(key).cloned() };
                if let Some(cached) = cached {
                    let reusable = {
                        let mut actor = cached.lock().expect("PointGet actor lock poisoned");
                        if actor.RecreatedFromPlan(point).is_ok() {
                            actor.SetDiagnosticMode(weak_consistency, redact_mode.clone());
                            true
                        } else {
                            false
                        }
                    };
                    if reusable {
                        self.effects
                            .borrow_mut()
                            .events
                            .push("point_get_cache_hit".into());
                        self.session.state.borrow_mut().statement_txn_start_ts = start_ts;
                        self.session
                            .WithSessionVars(|vars| vars.TxnCtx.SetStartTS(start_ts));
                        return Ok(Box::new(
                            astersql_executor::typed_point_get::SharedTypedPointGet::new(cached),
                        ));
                    }
                }
                let source = Arc::new(super::typed_adapter_bridge::OwnedKVSnapshotSource::new(
                    Arc::clone(&self.session.domain),
                    binding.version,
                ));
                let mut fresh = astersql_executor::builder::BuildTypedPointGet(
                    point,
                    source,
                    binding.initial_capacity,
                    binding.maximum_chunk_size,
                )
                .map_err(|error| errors::New(error.to_string()))?;
                fresh.SetDiagnosticMode(weak_consistency, redact_mode.clone());
                let actor = Arc::new(std::sync::Mutex::new(fresh));
                self.point_cache
                    .borrow_mut()
                    .insert(key.to_owned(), Arc::clone(&actor));
                return Ok(Box::new(
                    astersql_executor::typed_point_get::SharedTypedPointGet::new(actor),
                ));
            }
        }
        let source = Arc::new(super::typed_adapter_bridge::OwnedKVSnapshotSource::new(
            Arc::clone(&self.session.domain),
            binding.version,
        ));
        let mut executor = astersql_executor::builder::BuildTypedPointGet(
            point,
            source,
            binding.initial_capacity,
            binding.maximum_chunk_size,
        )
        .map_err(|error| errors::New(error.to_string()))?;
        executor.SetDiagnosticMode(weak_consistency, redact_mode);
        if self.session.state.borrow().transaction.is_some() {
            let read_committed = self
                .session
                .state
                .borrow()
                .transaction_isolation
                .eq_ignore_ascii_case("READ-COMMITTED");
            executor.SetLockRuntime(Rc::new(SessionPointLockRuntime {
                session: Rc::clone(&self.session),
                read_ts: start_ts,
                read_committed,
            }));
        }
        Ok(Box::new(executor))
    }
    fn RebuildPlan(
        &self,
        statement: &StatementNode,
    ) -> AdapterResult<(PlanInfo, Vec<FieldName>, i64)> {
        Err(errors::New("plan rebuild requires the canonical optimizer"))
    }
    fn NewChunk(&self, config: &ChunkConfig) -> chunk::Chunk {
        *chunk::New(
            config.fields.clone(),
            config.initial_capacity,
            config.maximum_chunk_size,
        )
    }
    fn StatementReadTS(&self) -> AdapterResult<u64> {
        Ok(self
            .scan
            .borrow()
            .as_ref()
            .ok_or_else(|| errors::New("typed scan was not bound"))?
            .version
            .Ver)
    }
    fn TransactionStartTS(&self) -> u64 {
        if self.dml_sql.borrow().is_some() {
            return self.session.WithSessionVars(|vars| vars.TxnCtx.StartTS());
        }
        self.scan
            .borrow()
            .as_ref()
            .map_or(0, |scan| scan.version.Ver)
    }
    fn SnapshotTS(&self) -> u64 {
        self.session
            .state
            .borrow()
            .snapshot_read_ts
            .unwrap_or_default()
    }
    fn LowResolutionTSO(&self) -> bool {
        let state = self.session.state.borrow();
        state.low_resolution_tso
            && !state.in_restricted_sql
            && self
                .session
                .connection_id
                .load(std::sync::atomic::Ordering::Acquire)
                > 0
    }
    fn IsPessimistic(&self) -> bool {
        self.session.TransactionIsPessimistic()
    }
    fn SupportsSelectForUpdate(&self) -> bool {
        if self.session.TransactionIsPessimistic()
            && self.physical_scan.borrow().as_ref().is_some_and(|binding| {
                binding
                    .plan
                    .as_plan()
                    .as_any()
                    .downcast_ref::<astersql_planner_core_operator_physicalop::PointGetPlan>()
                    .is_some_and(|point| point.Lock)
            })
        {
            return true;
        }
        if !self.session.TransactionIsPessimistic() || self.scan.borrow().is_none() {
            return false;
        }
        self.physical_scan.borrow().as_ref().is_some_and(|binding| {
            let plan = binding.plan.as_plan();
            if let Some(scan) =
                plan.as_any()
                    .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableScan>()
            {
                return scan.FilterCondition.is_empty()
                    && !scan.IsCommonHandle
                    && scan
                        .Table
                        .as_ref()
                        .is_some_and(|table| table.Indices.is_empty());
            }
            if let Some(reader) = super::typed_adapter_bridge::find_index_lookup(plan) {
                let mut scans = Vec::new();
                super::typed_adapter_bridge::collect_table_scans(plan, &mut scans);
                return scans.len() == 1 && !scans[0].IsCommonHandle;
            }
            false
        })
    }
    fn SupportsPreparedExecution(&self) -> bool {
        self.physical_scan
            .borrow()
            .as_ref()
            .is_some_and(|binding| binding.plan.is_prepared())
    }
    fn RunawayBeforeExecutor(
        &self,
        statement: &StatementNode,
        sql_digest: &str,
        plan_digest: &str,
    ) -> AdapterResult {
        self.runaway_checker.borrow_mut().take();
        {
            let mut state = self.session.state.borrow_mut();
            state.runaway_checker.take();
            state.runaway_resource_group_name.take();
        }
        if !astersql_sessionctx_vardef::EnableResourceControl.Load() {
            return Ok(());
        }
        let Some(manager) = self.session.domain.runaway_manager() else {
            return Ok(());
        };
        let group = self.ResourceGroupName();
        let started = self
            .effects
            .borrow()
            .process_started
            .unwrap_or_else(SystemTime::now);
        let start_us = started
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
            .min(i64::MAX as u128) as i64;
        let Some(mut checker) = manager.DeriveChecker(
            &group,
            statement.original_text.clone(),
            sql_digest.into(),
            plan_digest.into(),
            start_us,
        ) else {
            return Ok(());
        };
        let switched = checker
            .BeforeExecutor()
            .map_err(|error| errors::New(error.to_string()))?;
        if !switched.is_empty() {
            *self.runaway_resource_group_override.borrow_mut() = Some(switched.clone());
            self.session.state.borrow_mut().runaway_resource_group_name = Some(switched);
        } else {
            self.session.state.borrow_mut().runaway_resource_group_name = Some(group);
        }
        let checker = Arc::new(checker);
        self.session.state.borrow_mut().runaway_checker = Some(Arc::new(
            super::typed_runaway_checker::SessionRunawayChecker(Arc::clone(&checker)),
        ));
        *self.runaway_checker.borrow_mut() = Some(checker);
        Ok(())
    }
    fn ForeignKeyChecks(&self) -> bool {
        self.session.state.borrow().foreign_key_checks
    }
    fn StmtCommit(&self) -> AdapterResult {
        if self.session.state.borrow().transaction.is_none() {
            return Err(errors::New("canonical DML transaction is not active"));
        }
        // Canonical Transaction::Set/Delete already publishes writes to its
        // transaction mem-buffer, so the next FK batch reads this same overlay.
        self.effects
            .borrow_mut()
            .events
            .push("fk_stmt_commit".into());
        Ok(())
    }
    fn ForeignKeySavepointName(&self) -> String {
        self.fk_savepoint.borrow().clone().unwrap_or_default()
    }
    fn ReleaseForeignKeySavepoint(&self, savepoint: &str) {
        if savepoint.is_empty() {
            return;
        }
        let statement = astersql_parser_ast::ReleaseSavepointStmt {
            Name: savepoint.to_owned(),
            ..Default::default()
        };
        if let Err(error) = self.session.release_savepoint(&statement) {
            self.effects
                .borrow_mut()
                .events
                .push(format!("fk_savepoint_release_error:{error}"));
        }
        let matches_current = self.fk_savepoint.borrow().as_deref() == Some(savepoint);
        if matches_current {
            self.fk_savepoint.borrow_mut().take();
        }
    }
    fn SetInHandleForeignKeyTrigger(&self, active: bool) {
        self.session.WithSessionVars(|vars| {
            vars.StmtCtx
                .InHandleForeignKeyTrigger
                .store(active, std::sync::atomic::Ordering::Relaxed);
        });
    }
    fn ForeignKeyCheckInSharedLock(&self) -> bool {
        self.session.state.borrow().foreign_key_check_in_shared_lock
    }
    fn KillSignal(&self) -> AdapterResult {
        if self.deadline.borrow().as_ref().is_some_and(|deadline| {
            deadline.started.elapsed().unwrap_or_default()
                >= Duration::from_millis(deadline.maximum_ms)
        }) {
            self.session
                .SQLKiller()
                .SendKillSignal(astersql_util_sqlkiller::sqlkiller::MaxExecTimeExceeded);
        }
        SessionBoundAdapterOwner::KillSignal(self)
    }
    fn SQLKillerHandle(
        &self,
    ) -> Option<std::sync::Arc<astersql_util_sqlkiller::sqlkiller::SQLKiller>> {
        Some(self.session.SQLKiller())
    }
    fn CurrentDatabase(&self) -> String {
        SessionBoundAdapterOwner::CurrentDatabase(self)
    }
    fn InitialChunkSize(&self) -> usize {
        self.scan
            .borrow()
            .as_ref()
            .map_or(0, |scan| scan.initial_capacity)
    }
    fn MaximumChunkSize(&self) -> usize {
        self.scan
            .borrow()
            .as_ref()
            .map_or(0, |scan| scan.maximum_chunk_size)
    }
    fn Command(&self) -> u8 {
        self.session.WithSessionVars(|vars| vars.CommandValue)
    }
    fn MaximumExecutionTime(&self) -> u64 {
        self.session.WithSessionVars(|vars| {
            vars.GetSystemVar("max_execution_time")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        })
    }
    fn SetProcessInfo(&self, sql: &str, started: SystemTime, command: u8, maximum_time: u64) {
        self.session
            .WithSessionVars(|vars| vars.StmtCtx.ResetAdapterExecRetryCount());
        *self.deadline.borrow_mut() = (maximum_time > 0).then(|| {
            super::typed_adapter_bridge::ExecutionDeadline::new(
                self.session.SQLKiller(),
                started,
                maximum_time,
            )
        });
        let mut effects = self.effects.borrow_mut();
        effects.process_sql = sql.to_owned();
        effects.process_started = Some(started);
        effects
            .events
            .push(format!("process:{command}:{maximum_time}:{started:?}"));
    }
    fn CancelMaximumExecutionTime(&self) {
        self.deadline.borrow_mut().take();
    }
    fn FinalizePreparedExecution(&self, scanned_rows: usize, success: bool) {
        if let Some(binding) = self.physical_scan.borrow_mut().as_mut() {
            if let super::typed_adapter_bridge::BoundPhysicalPlan::Prepared(planned) =
                &mut binding.plan
            {
                if success {
                    self.session
                        .FinishPreparedKVPhysicalPlan(planned, scanned_rows);
                } else {
                    planned.PendingCache.take();
                }
            }
        }
    }
    fn SetPriority(&self, priority: Priority) {
        self.statement_context.borrow_mut().priority = priority;
    }
    fn SetLastFoundRows(&self, rows: u64) {
        self.session
            .WithSessionVars(|vars| vars.SetLastFoundRows(rows));
        self.effects.borrow_mut().last_found_rows = rows;
    }
    fn AddFoundRows(&self, rows: u64) {
        self.session
            .WithSessionVars(|vars| vars.StmtCtx.AddFoundRows(rows));
        self.effects
            .borrow_mut()
            .events
            .push(format!("found_rows:{rows}"));
    }
    fn ResetStatementForRetry(&self) {
        self.statement_context.borrow_mut().found_rows = 0;
        self.session.WithSessionVars(|vars| {
            vars.StmtCtx.ResetForRetry();
            vars.RetryInfo.ResetOffset();
        });
        self.effects
            .borrow_mut()
            .events
            .push("stmt_retry_reset".into());
    }
    fn InheritExecuteStatement(&self, statement: &mut StatementNode) -> AdapterResult {
        let sql = self.PreparedStatementSQL();
        if sql.is_empty() {
            return Err(errors::New("canonical prepared physical plan is not bound"));
        }
        statement.kind = StatementKind::Select;
        statement.prepared_text = Some(sql.clone());
        statement.text = sql.clone();
        statement.secure_text = sql;
        Ok(())
    }
    fn PreparedStatementSQL(&self) -> String {
        self.physical_scan
            .borrow()
            .as_ref()
            .and_then(|binding| match &binding.plan {
                super::typed_adapter_bridge::BoundPhysicalPlan::Prepared(planned) => {
                    Some(planned.SQLText.clone())
                }
                super::typed_adapter_bridge::BoundPhysicalPlan::Plain(_) => None,
            })
            .unwrap_or_default()
    }
    fn IsReadOnly(&self, statement: &StatementNode) -> bool {
        statement.kind == StatementKind::Select
    }
    fn PrepareFKCascadeContext(&self) {
        static NEXT_FK_SAVEPOINT: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        if self.session.state.borrow().transaction.is_none() {
            return;
        }
        let name = format!(
            "aster_adapter_fk_{}",
            NEXT_FK_SAVEPOINT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let statement = astersql_parser_ast::SavepointStmt {
            Name: name.clone(),
            ..Default::default()
        };
        match self.session.create_savepoint(&statement) {
            Ok(()) => *self.fk_savepoint.borrow_mut() = Some(name),
            Err(error) => self
                .effects
                .borrow_mut()
                .events
                .push(format!("fk_savepoint_prepare_error:{error}")),
        }
    }
    fn HandleFKTriggerError(&self) -> AdapterResult {
        let Some(savepoint) = self.fk_savepoint.borrow_mut().take() else {
            return Ok(());
        };
        self.session
            .rollback_to_savepoint(&savepoint)
            .map_err(|error| errors::New(error.to_string()))?;
        self.session
            .release_savepoint(&astersql_parser_ast::ReleaseSavepointStmt {
                Name: savepoint,
                ..Default::default()
            })
            .map_err(|error| errors::New(error.to_string()))
    }
    fn DetachTrackers(&self) {
        self.effects
            .borrow_mut()
            .events
            .push("detach_trackers".into());
    }
    fn ResetCTEStorage(&self) -> AdapterResult {
        self.session.cte_scopes.borrow_mut().clear();
        Ok(())
    }
    fn OnPessimisticStmtStart(&self) -> AdapterResult {
        if !self.session.TransactionIsPessimistic() {
            return Err(errors::New("no active canonical pessimistic transaction"));
        }
        self.session
            .end_expired_pessimistic_transaction()
            .map_err(|error| errors::New(error.to_string()))?;
        if self.session.state.borrow().fair_locking {
            self.session
                .state
                .borrow_mut()
                .transaction
                .as_mut()
                .expect("active pessimistic transaction")
                .StartFairLocking()
                .map_err(|error| errors::New(error.to_string()))?;
        }
        let before = self.session.state.borrow().held_row_locks.clone();
        *self.statement_locks_before.borrow_mut() = Some(before);
        self.begin_statement_mutation_stage()?;
        self.session
            .begin_txn_statement_observation(&self.effects.borrow().process_sql);
        Ok(())
    }
    fn OnPessimisticStmtEnd(&self, success: bool) -> AdapterResult {
        self.finish_statement_mutation_stage(success)?;
        if let Some(transaction) = self.session.state.borrow_mut().transaction.as_mut()
            && transaction.IsInFairLockingMode()
        {
            let result = if success {
                transaction.DoneFairLocking(&astersql_kv::Context::default())
            } else {
                transaction.CancelFairLocking(&astersql_kv::Context::default())
            };
            result.map_err(|error| errors::New(error.to_string()))?;
        }
        if let Some(before) = self.statement_locks_before.borrow_mut().take() {
            if !success {
                let acquired = self
                    .session
                    .state
                    .borrow()
                    .held_row_locks
                    .difference(&before)
                    .cloned()
                    .collect::<std::collections::HashSet<_>>();
                super::release_runtime_row_locks(self.session.row_lock_owner, Some(&acquired));
                self.session
                    .state
                    .borrow_mut()
                    .held_row_locks
                    .retain(|key| !acquired.contains(key));
            }
        }
        Ok(())
    }
    fn AbortLazyUniquenessOnPessimisticDMLFailure(
        &self,
        error: &errors::SharedError,
    ) -> AdapterResult<Option<errors::SharedError>> {
        let state = self.session.state.borrow();
        let lazy_check = !state.constraint_check_in_place_pessimistic;
        let in_txn = state.transaction.is_some();
        drop(state);
        if !lazy_check || !in_txn {
            return Ok(None);
        }
        self.session
            .finish_transaction(false)
            .map_err(|rollback_error| errors::New(rollback_error.to_string()))?;
        Ok(Some(
            astersql_util_dbterror_exeerrors::exeerrors::ErrLazyUniquenessCheckFailure
                .GenWithStackByArgs(&[errors::ErrorArg::String(error.to_string())]),
        ))
    }
    fn PessimisticTransaction(&self) -> AdapterResult<Box<dyn pessimisticTxn>> {
        if !self.session.TransactionIsPessimistic() {
            return Err(errors::New("no active canonical pessimistic transaction"));
        }
        Ok(Box::new(SessionPessimisticTransaction {
            session: Rc::clone(&self.session),
            writes_before: self.session.state.borrow().transaction_write_keys.clone(),
        }))
    }
    fn ResetUnchangedKeysForLock(&self) {
        self.session
            .WithSessionVars(|vars| vars.TxnCtx.ResetUnchangedKeysForLock());
    }
    fn CollectUnchangedKeysForXLock(&self, keys: Vec<Key>) -> Vec<Key> {
        self.session
            .WithSessionVars(|vars| vars.TxnCtx.CollectUnchangedKeysForXLock(keys))
    }
    fn CollectUnchangedKeysForSLock(&self, keys: Vec<Key>) -> Vec<Key> {
        self.session
            .WithSessionVars(|vars| vars.TxnCtx.CollectUnchangedKeysForSLock(keys))
    }
    fn LockKeys(&self, keys: &[Key], shared: bool) -> AdapterResult {
        if !self.session.TransactionIsPessimistic() {
            return Err(errors::New(
                "locking keys requires an active canonical pessimistic transaction",
            ));
        }
        let temporary_ids = {
            let state = self.session.state.borrow();
            state
                .local_temporary_tables
                .values()
                .map(|table| table.ID)
                .chain(state.global_temporary_tables_in_transaction.keys().copied())
                .collect::<std::collections::HashSet<_>>()
        };
        let locked_ids = self
            .session
            .WithSessionVars(|vars| vars.StmtCtx.LogicalPlanLockTableIDs());
        let keys = keys
            .iter()
            .filter(|key| {
                let table_id = astersql_tablecodec::DecodeTableID(astersql_tablecodec::kv::Key(
                    (**key).clone(),
                ));
                !temporary_ids.contains(&table_id)
                    && (locked_ids.is_empty() || locked_ids.contains(&table_id))
            })
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(());
        }
        if !shared && self.retry_lock_conflict_once.replace(false) {
            return Err(astersql_kv::ErrWriteConflict.FastGenByArgs(&[]));
        }
        let lock_count = i32::try_from(keys.len()).unwrap_or(i32::MAX);
        let lock_started = std::time::Instant::now();
        if self.statement_locks_before.borrow().is_some()
            && let Some(read_ts) = self.scan.borrow().as_ref().map(|scan| scan.version.Ver)
        {
            let current = self
                .session
                .domain
                .storage()
                .with_storage(|storage| storage.CurrentVersion("global"))
                .map_err(|error| errors::New(error.to_string()))?;
            for key in keys.iter().copied() {
                if !astersql_tablecodec::IsRecordKey(key) {
                    continue;
                }
                let latest = self.session.domain.storage().with_storage(|storage| {
                    storage.GetSnapshot(current).Get(
                        &astersql_kv::Context::default(),
                        astersql_kv::Key(key.clone()),
                        &[],
                    )
                });
                match latest {
                    Ok(value) if value.CommitTs > read_ts => {
                        return Err(astersql_kv::ErrWriteConflict.FastGenByArgs(&[]));
                    }
                    Err(error) if astersql_kv::ErrNotExist.Equal(Some(&error)) => {
                        return Err(astersql_kv::ErrWriteConflict.FastGenByArgs(&[]));
                    }
                    Err(error) => return Err(error),
                    Ok(_) => {}
                }
            }
        }
        let domain_id = std::sync::Arc::as_ptr(&self.session.domain) as usize;
        let keys = keys
            .into_iter()
            .cloned()
            .map(|key| super::RuntimeRowLockKey { domain_id, key })
            .collect::<Vec<_>>();
        let result = if shared {
            self.session.acquire_shared_row_locks(keys)
        } else {
            self.session.acquire_row_locks(keys, false, None, true)
        };
        let details = astersql_util_execdetails::execdetails::util::LockKeysDetails {
            TotalTime: lock_started.elapsed(),
            LockKeys: lock_count,
            ..Default::default()
        };
        self.session.WithSessionVars(|vars| {
            if shared {
                vars.StmtCtx
                    .SyncExecDetails
                    .MergeSharedLockKeysExecDetails(Some(details));
            } else {
                vars.StmtCtx
                    .SyncExecDetails
                    .MergeLockKeysExecDetails(Some(details));
            }
        });
        result.map_err(|error| errors::New(error.to_string()))
    }
    fn OnPessimisticLockError(
        &self,
        error: &errors::SharedError,
    ) -> AdapterResult<PessimisticErrorAction> {
        Ok(if astersql_kv::ErrWriteConflict.Equal(Some(error)) {
            PessimisticErrorAction::RetryReady
        } else {
            PessimisticErrorAction::ReturnError
        })
    }
    fn OnPessimisticStmtRetry(&self) -> AdapterResult {
        if let Some(transaction) = self.session.state.borrow_mut().transaction.as_mut()
            && transaction.IsInFairLockingMode()
        {
            transaction
                .RetryFairLocking(&astersql_kv::Context::default())
                .map_err(|error| errors::New(error.to_string()))?;
        }
        let version = self
            .session
            .domain
            .storage()
            .with_storage(|storage| storage.CurrentVersion("global"))
            .map_err(|error| errors::New(error.to_string()))?;
        if let Some(scan) = self.scan.borrow_mut().as_mut() {
            scan.version = version;
        }
        if let Some(binding) = self.physical_scan.borrow_mut().as_mut() {
            binding.version = version;
        }
        self.effects
            .borrow_mut()
            .events
            .push(format!("pessimistic_retry_read_ts:{}", version.Ver));
        Ok(())
    }
    fn RollbackStatementForRetry(&self) -> AdapterResult {
        self.finish_statement_mutation_stage(false)?;
        let mut released = 0;
        if let Some(before) = self.statement_locks_before.borrow().as_ref() {
            let acquired = self
                .session
                .state
                .borrow()
                .held_row_locks
                .difference(before)
                .cloned()
                .collect::<std::collections::HashSet<_>>();
            released = acquired.len();
            super::release_runtime_row_locks(self.session.row_lock_owner, Some(&acquired));
            self.session
                .state
                .borrow_mut()
                .held_row_locks
                .retain(|key| !acquired.contains(key));
        }
        self.effects
            .borrow_mut()
            .events
            .push(format!("pessimistic_retry_rollback:{released}"));
        self.begin_statement_mutation_stage()?;
        Ok(())
    }
    fn MaximumPessimisticRetries(&self) -> usize {
        usize::try_from(
            astersql_config::get_global_config()
                .pessimistic_txn
                .max_retry_count,
        )
        .unwrap_or(usize::MAX)
    }
    fn StatementContext(&self) -> StatementContext {
        self.statement_context.borrow().clone()
    }
    fn SetStatementContext(&self, context: &StatementContext) {
        *self.statement_context.borrow_mut() = context.clone();
    }
    fn Digest(&self, text: &str) -> Digest {
        let parser_digest = astersql_parser::DigestNormalized(text);
        Digest {
            text: parser_digest.String().to_owned(),
            bytes: parser_digest.Bytes().to_vec(),
        }
    }
    fn Audit(&self, sql: &str) {
        if self.RestrictedSQL() {
            return;
        }
        let tables = self
            .scan
            .borrow()
            .as_ref()
            .and_then(|scan| {
                self.session
                    .domain
                    .info_schema()
                    .TableItemByID(scan.table_id)
            })
            .map(|table| {
                vec![plugin::TableEntry {
                    db: table.DBName.original,
                    table: table.TableName.original,
                }]
            })
            .unwrap_or_default();
        let (session_view, retrying, command) = self.session.WithSessionVars(|vars| {
            (
                plugin::SessionVars {
                    connection_id: vars.ConnectionID,
                    original_sql: sql.to_owned(),
                    stmt_type: self.statement_context.borrow().statement_type.clone(),
                    affected_rows: self.statement_context.borrow().affected_rows,
                    tables,
                    status: vars.Status,
                    user: vars
                        .User
                        .as_ref()
                        .map_or_else(String::new, |user| user.username.clone()),
                },
                vars.RetryInfo.Retrying,
                vars.CommandValue,
            )
        });
        let started = self
            .effects
            .borrow()
            .process_started
            .unwrap_or_else(SystemTime::now);
        let context = plugin::Context::default()
            .with_value(plugin::EXEC_START_TIME_CONTEXT_KEY, started)
            .with_value(plugin::IS_RETRYING_CONTEXT_KEY, retrying);
        let command_name = astersql_parser_mysql::r#const::Command2Str
            .iter()
            .find_map(|(code, name)| (*code == command).then_some(*name))
            .unwrap_or("UNKNOWN");
        if let Err(error) = plugin::foreach_plugin(plugin::Kind::Audit, |loaded| {
            if let Some(callback) = plugin::audit_callbacks(&loaded.manifest)
                .and_then(|callbacks| callbacks.on_general_event.as_ref())
            {
                callback(
                    &context,
                    Some(&session_view),
                    plugin::GeneralEvent::Completed,
                    command_name,
                );
            }
            Ok(())
        }) {
            self.effects
                .borrow_mut()
                .events
                .push(format!("audit_error:{error}"));
        }
        self.effects.borrow_mut().audited_sql.push(sql.to_owned());
    }
    fn ObservePhase(&self, phase: &str, internal: bool, duration: Duration) {
        self.effects
            .borrow_mut()
            .events
            .push(format!("phase:{phase}:{internal}:{duration:?}"));
    }
    fn RecordDMLMetric(&self, statement_type: &str, value: i64) {
        self.effects
            .borrow_mut()
            .events
            .push(format!("dml:{statement_type}:{value}"));
    }
    fn RUV2Weights(&self) -> RUV2Weights {
        self.session.WithSessionVars(|vars| vars.RUV2Weights())
    }
    fn RUV2ReporterAvailable(&self) -> bool {
        self.session.domain.ruv2_consumption_reporter().is_some()
    }
    fn ResourceGroupName(&self) -> String {
        self.runaway_resource_group_override
            .borrow()
            .clone()
            .unwrap_or_else(|| {
                let name = self
                    .session
                    .WithSessionVars(|vars| vars.StmtCtx.ResourceGroupName.clone());
                if name.is_empty() {
                    astersql_resourcegroup::DEFAULT_RESOURCE_GROUP_NAME.into()
                } else {
                    name
                }
            })
    }
    fn ReportRUV2Consumption(&self, resource_group: &str, tikv: f64, tidb: f64, tiflash: f64) {
        self.effects
            .borrow_mut()
            .events
            .push(format!("ruv2:{resource_group}:{tikv}:{tidb}:{tiflash}"));
        if let Some(reporter) = self.session.domain.ruv2_consumption_reporter() {
            reporter.report_ruv2_consumption(resource_group, tikv, tidb, tiflash);
        }
    }
    fn RecordLastQuery(&self, error: Option<&str>) {
        self.effects.borrow_mut().last_error = error.map(str::to_owned);
    }
    fn PlanReplayerCapture(&self, statement: &StatementNode, start_ts: u64, continuous: bool) {
        self.effects.borrow_mut().events.push(format!(
            "plan_replayer:{}:{start_ts}:{continuous}",
            statement.text
        ));
    }
    fn SlowQuery(&self, transaction_ts: u64, sql: &str, success: bool, has_more_results: bool) {
        let mut effects = self.effects.borrow_mut();
        effects.slow_queries.push((sql.to_owned(), success));
        effects
            .events
            .push(format!("slow:{transaction_ts}:{has_more_results}"));
        let Some(started) = effects.process_started else {
            return;
        };
        let duration = started.elapsed().unwrap_or_default();
        let duration_ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        let threshold_ms = self.session.WithSessionVars(|vars| {
            vars.GetSystemVar("tidb_slow_log_threshold")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(300)
        });
        if duration_ms >= threshold_ms {
            self.session
                .domain
                .log_slow_query(astersql_domain::domain::SlowQueryInfo {
                    sql: sql.to_owned(),
                    digest: self.statement_context.borrow().sql_digest.text.clone(),
                    duration_ms,
                    start: started,
                    internal: self.RestrictedSQL(),
                });
        }
    }
    fn Summary(&self, summary: &StatementSummary) {
        self.effects
            .borrow_mut()
            .summaries
            .push(summary.original_sql.clone());
        let started = self
            .effects
            .borrow()
            .process_started
            .unwrap_or_else(SystemTime::now);
        let previous = self
            .effects
            .borrow()
            .previous_statement
            .clone()
            .unwrap_or_default();
        let (stmt_type, affected, detail, charset, collation) =
            self.session.WithSessionVars(|vars| {
                (
                    vars.StmtCtx.StmtType.clone(),
                    vars.StmtCtx.AffectedRows(),
                    vars.StmtCtx.GetExecDetails(),
                    vars.GetSystemVar("character_set_connection")
                        .unwrap_or_default(),
                    vars.GetSystemVar("collation_connection")
                        .unwrap_or_default(),
                )
            });
        let mut info = astersql_util_stmtsummary::StmtExecInfo {
            SchemaName: self.session.current_database(),
            Charset: charset,
            Collation: collation,
            NormalizedSQL: summary.normalized_sql.clone(),
            Digest: summary.sql_digest.clone(),
            PlanDigest: summary.plan_digest.clone(),
            PrevSQL: previous.0,
            PrevSQLDigest: previous.1,
            TotalLatency: started.elapsed().unwrap_or_default(),
            StartTime: started,
            IsInternal: self.RestrictedSQL(),
            Succeed: summary.success,
            ExecDetail: detail,
            ResourceGroupName: self.ResourceGroupName(),
            LazyInfo: Box::new(AdapterSummaryLazyInfo(summary.clone())),
            ..Default::default()
        };
        info.StmtCtx.StmtType = stmt_type;
        info.StmtCtx.SetAffectedRows(affected);
        astersql_util_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("statement summary map lock poisoned")
            .AddStatement(&info);
    }
    fn UpdatePreviousStatement(&self, sql: &str, digest: &str) {
        self.effects.borrow_mut().previous_statement = Some((sql.to_owned(), digest.to_owned()));
    }
    fn RecordNetworkTraffic(&self, sent: u64, received: u64, mpp: u64) {
        self.effects
            .borrow_mut()
            .events
            .push(format!("network:{sent}:{received}:{mpp}"));
    }
    fn RecordPlanCache(&self, hit: bool, reason: Option<&str>) {
        self.effects
            .borrow_mut()
            .events
            .push(format!("plan_cache:{hit}:{reason:?}"));
    }
    fn TopSQLStart(&self, sql_digest: &[u8], plan_digest: &[u8]) {
        let mut effects = self.effects.borrow_mut();
        effects.top_sql_started += 1;
        effects.events.push(format!(
            "top_sql_start:{}:{}",
            sql_digest.len(),
            plan_digest.len()
        ));
        drop(effects);
        let existing = { self.top_sql_stats.borrow().as_ref().cloned() };
        let stats = existing.unwrap_or_else(|| {
            let stats = astersql_util_topsql_stmtstats::CreateStatementStats();
            *self.top_sql_stats.borrow_mut() = Some(Arc::clone(&stats));
            stats
        });
        let sc = self.statement_context.borrow();
        let top_ru = astersql_util_topsql_state::TopRUEnabled();
        let begin = astersql_util_topsql_stmtstats::ExecBeginInfo {
            InNetworkBytes: sc.network_received_bytes,
            TopRUEnabled: top_ru,
            ..Default::default()
        };
        stats.OnExecutionBegin(sql_digest, plan_digest, Some(&begin));
        *self.top_sql_current.borrow_mut() =
            Some((sql_digest.to_vec(), plan_digest.to_vec(), SystemTime::now()));
        if astersql_util_topsql::TopProfilingReporterAvailable() {
            if !sql_digest.is_empty() {
                let digest = astersql_parser::Digest::new(sql_digest.to_vec());
                astersql_util_topsql::RegisterSQL(
                    &sc.sql_normalized,
                    Some(&digest),
                    self.RestrictedSQL(),
                );
            }
            if !plan_digest.is_empty() {
                let digest = astersql_parser::Digest::new(plan_digest.to_vec());
                let normalized = sc
                    .plan_digest
                    .as_ref()
                    .map(|(normalized, _)| normalized.clone())
                    .unwrap_or_default();
                astersql_util_topsql::RegisterPlan(normalized, Some(&digest));
            }
        }
    }
    fn TopSQLFinish(&self) {
        self.deadline.borrow_mut().take();
        self.effects.borrow_mut().top_sql_finished += 1;
        let Some((sql_digest, plan_digest, started)) = self.top_sql_current.borrow_mut().take()
        else {
            return;
        };
        let Some(stats) = self.top_sql_stats.borrow().as_ref().cloned() else {
            return;
        };
        let duration_ns = started
            .elapsed()
            .unwrap_or_default()
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        let finish = astersql_util_topsql_stmtstats::ExecFinishInfo {
            OutNetworkBytes: self.statement_context.borrow().network_sent_bytes,
            ExecDuration: astersql_util_topsql_stmtstats::SignedDuration::from_nanos(duration_ns),
            TopRUEnabled: astersql_util_topsql_state::TopRUEnabled(),
            ..Default::default()
        };
        stats.OnExecutionFinished(&sql_digest, &plan_digest, Some(&finish));
    }
    fn OnFinishStatement(&self, retries: usize, success: bool, affected_rows: u64) {
        use std::sync::atomic::Ordering;
        let (processed_keys, write_size, write_keys) = self.session.WithSessionVars(|vars| {
            vars.StmtCtx
                .AddAdapterExecRetryCount(retries.min(u64::MAX as usize) as u64);
            let details = vars.StmtCtx.GetExecDetails();
            let processed_keys = details
                .CopExecDetails
                .ScanDetail
                .as_ref()
                .map(|scan| scan.ProcessedKeys.max(0))
                .unwrap_or(0);
            if processed_keys > 0 {
                vars.KeysExamined
                    .fetch_add(processed_keys as u64, Ordering::AcqRel);
            }
            let (write_size, write_keys) = details
                .CommitDetail
                .as_ref()
                .map(|commit| (commit.WriteSize.max(0), commit.WriteKeys.max(0)))
                .unwrap_or((0, 0));
            (processed_keys, write_size, write_keys)
        });
        let mut state = self.session.state.borrow_mut();
        if affected_rows > 0 && processed_keys > 0 {
            state.txn_write_throughput_sli.AddReadKeys(processed_keys);
        }
        if write_size > 0 {
            state
                .txn_write_throughput_sli
                .AddTxnWriteSize(write_size as isize, write_keys as isize);
        }
        self.effects
            .borrow_mut()
            .events
            .push(format!("finish_retries:{retries}:success:{success}"));
    }
    fn CommitDetailsForFinish(&self) -> Option<astersql_executor::adapter::CommitDetails> {
        self.session.WithSessionVars(|vars| {
            let details = vars.StmtCtx.GetExecDetails();
            details
                .CommitDetail
                .map(|commit| astersql_executor::adapter::CommitDetails {
                    prewrite_time: commit.PrewriteTime,
                    commit_time: commit.CommitTime,
                    get_commit_ts_time: commit.GetCommitTsTime,
                    get_latest_ts_time: commit.GetLatestTsTime,
                    local_latch_time: commit.LocalLatchTime,
                    wait_prewrite_binlog_time: commit.WaitPrewriteBinlogTime,
                })
        })
    }
    fn AttachFinishRuntimeStats(&self, plan_id: i32) {
        self.session.WithSessionVars(|vars| {
            let details = vars.StmtCtx.GetExecDetails();
            if details.CommitDetail.is_none()
                && details.LockKeysDetail.is_none()
                && details.SharedLockKeysDetail.is_none()
            {
                return;
            }
            let Some(coll) = vars.StmtCtx.RuntimeStatsColl.as_ref() else {
                return;
            };
            coll.RegisterStatsShared(
                plan_id,
                Box::new(
                    astersql_util_execdetails::execdetails::RuntimeStatsWithCommit {
                        Commit: details.CommitDetail,
                        LockKeys: details.LockKeysDetail,
                        SharedLockKeys: details.SharedLockKeysDetail,
                        ..Default::default()
                    },
                ),
            );
        });
    }
    fn OnExecComplete(&self, success: bool) {
        use std::sync::atomic::Ordering;
        if !success {
            return;
        }
        self.session.WithSessionVars(|vars| {
            let details = vars.StmtCtx.GetExecDetails();
            let Some(lock) = details.LockKeysDetail else {
                return;
            };
            if lock.AggressiveLockNewCount > 0 || lock.AggressiveLockDerivedCount > 0 {
                vars.TxnCtx.FairLockingUsed.store(true, Ordering::Release);
                if lock.LockedWithConflictCount > 0 || lock.AggressiveLockDerivedCount > 0 {
                    vars.TxnCtx
                        .FairLockingEffective
                        .store(true, Ordering::Release);
                }
            }
        });
    }
    fn ExecLockMetrics(&self) -> astersql_executor::adapter::ExecLockMetrics {
        let metrics = self.session.WithSessionVars(|vars| {
            let details = vars.StmtCtx.GetExecDetails();
            astersql_executor::adapter::ExecLockMetrics {
                exclusive_keys: details
                    .LockKeysDetail
                    .as_ref()
                    .map(|detail| detail.LockKeys)
                    .unwrap_or(0),
                shared_keys: details
                    .SharedLockKeysDetail
                    .as_ref()
                    .map(|detail| detail.LockKeys)
                    .unwrap_or(0),
                exclusive_duration: details
                    .LockKeysDetail
                    .as_ref()
                    .map(|detail| detail.TotalTime)
                    .unwrap_or_default(),
                pessimistic_lock_started: vars.StmtCtx.PessimisticLockStarted(),
            }
        });
        self.effects.borrow_mut().events.push(format!(
            "exec_locks:{}:{}:{:?}:{}",
            metrics.exclusive_keys,
            metrics.shared_keys,
            metrics.exclusive_duration,
            metrics.pessimistic_lock_started
        ));
        metrics
    }
    fn FairLockingFinishMetrics(&self) -> astersql_executor::adapter::FairLockingFinishMetrics {
        use std::sync::atomic::Ordering;
        let metrics = self.session.WithSessionVars(|vars| {
            let details = vars.StmtCtx.GetExecDetails();
            let (stmt_used, stmt_effective) = details
                .LockKeysDetail
                .as_ref()
                .map(|lock| {
                    (
                        lock.AggressiveLockNewCount > 0 || lock.AggressiveLockDerivedCount > 0,
                        lock.LockedWithConflictCount > 0 || lock.AggressiveLockDerivedCount > 0,
                    )
                })
                .unwrap_or((false, false));
            let committed = details.CommitDetail.is_some();
            astersql_executor::adapter::FairLockingFinishMetrics {
                stmt_used,
                stmt_effective: stmt_used && stmt_effective,
                txn_used: committed && vars.TxnCtx.FairLockingUsed.load(Ordering::Acquire),
                txn_effective: committed
                    && vars.TxnCtx.FairLockingEffective.load(Ordering::Acquire),
            }
        });
        self.effects.borrow_mut().events.push(format!(
            "fair_locking:{}:{}:{}:{}",
            metrics.stmt_used, metrics.stmt_effective, metrics.txn_used, metrics.txn_effective
        ));
        metrics
    }
    fn SupplementaryFinishMetrics(&self) -> astersql_executor::adapter::SupplementaryFinishMetrics {
        use std::sync::atomic::Ordering;
        let metrics = self.session.WithSessionVars(|vars| {
            astersql_executor::adapter::SupplementaryFinishMetrics {
                tiflash: vars.StmtCtx.IsTiFlash.load(Ordering::Acquire),
                read_from_table_cache: vars.StmtCtx.IsReadFromTableCache(),
            }
        });
        self.effects.borrow_mut().events.push(format!(
            "supplementary:{}:{}",
            metrics.tiflash, metrics.read_from_table_cache
        ));
        metrics
    }
    fn ExecuteRunDurationForFinish(&self) -> Option<Duration> {
        let duration = self
            .session
            .WithSessionVars(|vars| vars.GetExecuteDuration());
        self.effects.borrow_mut().events.push(format!(
            "execute_run_duration:{}:{duration:?}",
            self.RestrictedSQL()
        ));
        Some(duration)
    }
    fn CleanupAfterFinish(&self) {
        use std::sync::atomic::Ordering;
        self.session.WithSessionVars(|vars| {
            let mpp = &vars.StmtCtx.MPPQueryInfo;
            mpp.QueryID.store(0, Ordering::Release);
            mpp.QueryTS.store(0, Ordering::Release);
            mpp.AllocatedMPPTaskID.store(0, Ordering::Release);
            mpp.AllocatedMPPGatherID.store(0, Ordering::Release);
            vars.StmtCtx.SetStaleness(false);
            vars.StmtCtx.IsTiFlash.store(false, Ordering::Release);
            vars.StmtCtx.ResetReadFromTableCache();
            vars.ResetDurationParse();
        });
        {
            let mut state = self.session.state.borrow_mut();
            state.runaway_checker.take();
            state.runaway_resource_group_name.take();
        }
        self.runaway_resource_group_override.borrow_mut().take();
    }
    fn RestrictedSQL(&self) -> bool {
        self.session.WithSessionVars(|vars| vars.InRestrictedSQL)
    }
    fn RedactLog(&self) -> bool {
        self.session.WithSessionVars(|vars| {
            vars.GetSystemVar("tidb_redact_log")
                .is_some_and(|value| value.eq_ignore_ascii_case("ON") || value == "1")
        })
    }
}

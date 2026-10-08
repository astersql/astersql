// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use super::session::RuntimeTimeZone;
use super::*;

fn default_resource_group_background(
    options: &[ast::ResourceGroupBackgroundOption],
) -> Option<astersql_meta_model::group_3::ResourceGroupBackgroundSettings> {
    use astersql_meta_model::group_3::ResourceGroupBackgroundSettings;

    let mut background = ResourceGroupBackgroundSettings::default();
    let mut has_option = false;
    for option in options {
        match option.Type {
            ast::BackgroundOptionType::TaskNames => {
                has_option = true;
                background.JobTypes = option
                    .StrValue
                    .split(',')
                    .map(|task_type| task_type.trim().to_ascii_lowercase())
                    .filter(|task_type| !task_type.is_empty())
                    .collect();
            }
            ast::BackgroundOptionType::UtilizationLimit => {
                has_option = true;
                background.ResourceUtilLimit = option.UintValue;
            }
        }
    }
    has_option.then_some(background)
}

fn persist_default_resource_group_background(
    domain: &Arc<Domain>,
    background: astersql_meta_model::group_3::ResourceGroupBackgroundSettings,
) -> SessionResult<()> {
    use astersql_meta_model::group_3::{ResourceGroupInfo, SchemaState, ast, unlimitedRURate};

    let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
    domain
        .storage_handle()
        .with_storage(|store| {
            kv::RunInNewTxn(&context, store, true, |_, transaction| {
                let key =
                    astersql_meta::transaction_meta_hash_key(b"ResourceGroups", b"ResourceGroup:1");
                let mut group = match transaction.Get(&context, key.clone(), &[]) {
                    Ok(value) => {
                        let payload = match value.Value.first() {
                            Some(0) => &value.Value[1..],
                            Some(b'{') => value.Value.as_slice(),
                            _ => {
                                return Err(kv::errors::New(
                                    "invalid default resource group metadata",
                                ));
                            }
                        };
                        serde_json::from_slice::<ResourceGroupInfo>(payload)
                            .map_err(|error| kv::errors::New(error.to_string()))?
                    }
                    Err(error) if kv::IsErrNotFound(&error) => {
                        let mut settings = astersql_meta_model::group_3::NewResourceGroupSettings();
                        settings.RURate = unlimitedRURate;
                        settings.BurstLimit = -1;
                        ResourceGroupInfo {
                            ResourceGroupSettings: settings,
                            ID: 1,
                            Name: ast::NewCIStr("default"),
                            State: SchemaState::Public,
                        }
                    }
                    Err(error) => return Err(error),
                };
                group.ResourceGroupSettings.Background = Some(Arc::new(background.clone()));
                let mut encoded = vec![0];
                encoded.extend(
                    serde_json::to_vec(&group)
                        .map_err(|error| kv::errors::New(error.to_string()))?,
                );
                transaction.Set(key, encoded)
            })
        })
        .map_err(|error| {
            SessionError::new(format!(
                "persist default resource group background: {error}"
            ))
        })
}

pub(super) fn load_default_resource_group_background(
    domain: &Arc<Domain>,
) -> SessionResult<Option<String>> {
    let background_result: Result<
        Option<Arc<astersql_meta_model::group_3::ResourceGroupBackgroundSettings>>,
        kv::errors::SharedError,
    > = domain.storage_handle().with_storage(|store| {
        let version = store
            .CurrentVersion("global")
            .map_err(|error| kv::errors::New(error.to_string()))?;
        let reader = astersql_meta::SnapshotReader::new(store.GetSnapshot(version));
        let group = reader
            .get_resource_group(1)
            .map_err(|error| kv::errors::New(error.to_string()))?;
        Ok(group.and_then(|group| group.ResourceGroupSettings.Background.clone()))
    });
    let background = background_result.map_err(|error| {
        SessionError::new(format!("load default resource group background: {error}"))
    })?;
    let Some(background) = background else {
        return Ok(None);
    };
    let mut settings = Vec::new();
    if !background.JobTypes.is_empty() {
        settings.push(format!("TASK_TYPES='{}'", background.JobTypes.join(",")));
    }
    if background.ResourceUtilLimit > 0 {
        settings.push(format!(
            "UTILIZATION_LIMIT={}",
            background.ResourceUtilLimit
        ));
    }
    Ok((!settings.is_empty()).then(|| settings.join(", ")))
}

impl ConcreteSession {
    /// Number of cached point reads in the active transaction snapshot.
    pub fn SnapCacheSizeForTest(&self) -> usize {
        self.state
            .borrow()
            .transaction
            .as_ref()
            .map_or(0, |transaction| transaction.GetSnapshot().SnapCacheSize())
    }

    /// Toggle the row value encoder used by subsequent DML writes.
    pub fn SetRowEncoderEnabledForTest(&self, enabled: bool) {
        self.state.borrow_mut().row_encoder_enabled = enabled;
    }

    /// Returns whether the active SQL transaction uses pessimistic semantics.
    pub fn TransactionIsPessimistic(&self) -> bool {
        let state = self.state.borrow();
        state.transaction.is_some() && state.transaction_pessimistic
    }

    /// Returns the active transaction isolation name.
    pub fn TransactionIsolation(&self) -> String {
        self.state.borrow().transaction_isolation.clone()
    }

    /// Number of process-local row locks held by this session.
    pub fn HeldRowLockCount(&self) -> usize {
        self.state
            .borrow()
            .held_row_locks
            .iter()
            .filter(|key| {
                astersql_tablecodec::DecodeKeyHead(astersql_tablecodec::kv::Key(key.key.clone()))
                    .is_ok_and(|(_, _, is_record)| is_record)
            })
            .count()
    }

    /// Number of sessions currently waiting for a process-local row lock.
    pub fn RuntimeLockWaitCount(&self) -> usize {
        RUNTIME_ROW_LOCKS
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .wait_for
            .len()
    }

    /// Number of retained deadlock edges exposed by information_schema.
    pub fn RuntimeDeadlockHistoryCount(&self) -> usize {
        RUNTIME_DEADLOCK_HISTORY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Clear process-local deadlock history, mirroring the Go test hook.
    pub fn ClearRuntimeDeadlockHistory(&self) {
        RUNTIME_DEADLOCK_HISTORY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Override the managed pessimistic-lock TTL for deterministic tests.
    pub fn SetPessimisticLockTTLForTest(&self, ttl: Duration) {
        self.state.borrow_mut().pessimistic_lock_ttl = ttl;
    }

    /// Force the active pessimistic lock set to be considered expired.
    pub fn ExpirePessimisticLocksForTest(&self) {
        let mut state = self.state.borrow_mut();
        state.pessimistic_lock_ttl_expired_for_test = true;
        state.pessimistic_lock_started = Some(
            Instant::now()
                .checked_sub(state.pessimistic_lock_ttl + Duration::from_millis(1))
                .unwrap_or_else(Instant::now),
        );
    }

    pub(super) fn end_expired_pessimistic_transaction(&self) -> SessionResult<()> {
        let expired = {
            let mut state = self.state.borrow_mut();
            let active = state.transaction.is_some()
                && state.transaction_pessimistic
                && state
                    .pessimistic_lock_started
                    .is_some_and(|started| started.elapsed() >= state.pessimistic_lock_ttl);
            if active && !state.pessimistic_lock_ttl_expired_for_test {
                // TiDB's managed lock TTL is renewed by a background heartbeat.
                // The in-process runtime has no background manager, so renew at
                // statement boundaries while the manager remains healthy.
                state.pessimistic_lock_started = Some(Instant::now());
            }
            active && state.pessimistic_lock_ttl_expired_for_test
        };
        if !expired {
            return Ok(());
        }
        self.finish_transaction(false)?;
        Err(SessionError::new(
            "[tikv:8220]TTL manager has timed out, pessimistic transaction has been rolled back",
        ))
    }

    pub(super) fn retriever_visible(
        retriever: &dyn kv::Retriever,
    ) -> SessionResult<BTreeMap<Vec<u8>, Vec<u8>>> {
        let mut iterator = retriever
            .Iter(kv::Key(Vec::new()), None)
            .map_err(|error| session_error("scan transaction for savepoint", error))?;
        let mut visible = BTreeMap::new();
        while iterator.Valid() {
            visible.insert(iterator.Key().0, iterator.Value());
            iterator
                .Next()
                .map_err(|error| session_error("advance savepoint snapshot", error))?;
        }
        iterator.Close();
        Ok(visible)
    }

    pub(super) fn transaction_visible(
        transaction: &dyn kv::Transaction,
    ) -> SessionResult<BTreeMap<Vec<u8>, Vec<u8>>> {
        Self::retriever_visible(transaction)
    }

    pub(crate) fn snapshots_differ_on_keys(
        base: &dyn kv::Snapshot,
        latest: &dyn kv::Snapshot,
        keys: &[Vec<u8>],
    ) -> SessionResult<bool> {
        if keys.is_empty() {
            return Ok(false);
        }
        let keys = keys.iter().cloned().map(kv::Key).collect::<Vec<_>>();
        let context = kv::Context::default();
        let base_values = base
            .BatchGet(&context, &keys, &[])
            .map_err(|error| session_error("batch read transaction conflict snapshot", error))?;
        let latest_values = latest
            .BatchGet(&context, &keys, &[])
            .map_err(|error| session_error("batch read latest conflict snapshot", error))?;
        Ok(keys.iter().any(|key| {
            let name = kv::KeyMapName(&key.0);
            base_values.get(&name).map(|entry| &entry.Value)
                != latest_values.get(&name).map(|entry| &entry.Value)
        }))
    }

    pub(super) fn transaction_mem_usage(
        transaction: &dyn kv::Transaction,
    ) -> SessionResult<(u64, u64)> {
        let mut iterator = transaction
            .GetMemBuffer()
            .Iter(kv::Key(Vec::new()), None)
            .map_err(|error| session_error("scan transaction mem-buffer", error))?;
        let mut keys = 0_u64;
        let mut bytes = 0_u64;
        while iterator.Valid() {
            keys += 1;
            bytes = bytes
                .saturating_add(iterator.Key().0.len() as u64 + iterator.Value().len() as u64 + 16);
            iterator
                .Next()
                .map_err(|error| session_error("advance transaction mem-buffer", error))?;
        }
        iterator.Close();
        Ok((keys, bytes))
    }

    pub(super) fn begin_txn_statement_observation(&self, sql: &str) {
        self.session_vars
            .SetStatementStartTime(std::time::Instant::now());
        let observed_sql = self
            .state
            .borrow()
            .observation_sql_override
            .clone()
            .unwrap_or_else(|| sql.to_owned());
        let digest = astersql_parser::NormalizeDigest(&observed_sql)
            .1
            .String()
            .to_owned();
        {
            let mut state = self.state.borrow_mut();
            state.current_statement_digest = digest.clone();
            state.last_query_string = observed_sql;
        }
        self.state.borrow_mut().pessimistic_pause_observed_statement = false;
        self.state.borrow_mut().current_stale_now_ts = None;
        let lowered = sql.trim().to_ascii_lowercase();
        let statement_is_read = lowered.starts_with("select")
            || lowered.starts_with("execute")
            || lowered.starts_with("explain");
        let prepared_is_stale = lowered
            .strip_prefix("execute ")
            .and_then(|name| name.split_whitespace().next())
            .and_then(|name| self.state.borrow().prepared_by_name.get(name).cloned())
            .is_some_and(|prepared| {
                prepared
                    .sql
                    .to_ascii_lowercase()
                    .contains(" as of timestamp ")
            });
        let session_stale = {
            let state = self.state.borrow();
            state.transaction_stale_read_ts.is_some()
                || state.pending_stale_read_ts.is_some()
                || state.session_stale_read_ts.is_some()
                || state.snapshot_read_ts.is_some()
        };
        let is_stale = statement_is_read
            && (lowered.contains(" as of timestamp ") || prepared_is_stale || session_stale);
        let injected = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/exector/assertStmtCtxIsStaleness",
        );
        let observation_error = injected.and_then(|expected| {
            expected.parse::<bool>().ok().filter(|expected| *expected != is_stale).map(|expected| {
                format!(
                    "StmtCtx.IsStaleness mismatch for `{sql}`: expected {expected}, got {is_stale}"
                )
            })
        });
        {
            let mut state = self.state.borrow_mut();
            state.current_statement_is_stale = is_stale;
            state.stale_statement_observation_error = observation_error;
        }
        self.WithSessionVars(|vars| vars.StmtCtx.SetStaleness(is_stale));
        let mut infos = RUNTIME_TXN_INFOS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(info) = infos.get_mut(&self.row_lock_owner) {
            info.current_sql_digest = digest.clone();
            info.state = "Running".to_owned();
            info.waiting_start_time = None;
            info.all_sql_digests.push(digest);
        }
    }

    pub(super) fn finish_txn_statement_observation(&self, _sql: &str) -> SessionResult<()> {
        {
            let mut state = self.state.borrow_mut();
            state.last_statement_was_stale = state.current_statement_is_stale;
            state.current_statement_is_stale = false;
        }
        if let Some(error) = self
            .state
            .borrow_mut()
            .stale_statement_observation_error
            .take()
        {
            return Err(SessionError::new(error));
        }
        let state = self.state.borrow();
        let Some(transaction) = state.transaction.as_ref() else {
            RUNTIME_TXN_INFOS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.row_lock_owner);
            return Ok(());
        };
        let mem_buffer_keys = state.txn_mem_buffer_keys;
        let mem_buffer_bytes = state.txn_mem_buffer_bytes;
        let digest = self.state.borrow().current_statement_digest.clone();
        let mut infos = RUNTIME_TXN_INFOS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let info = infos
            .entry(self.row_lock_owner)
            .or_insert_with(|| RuntimeTxnInfo {
                domain_id: Arc::as_ptr(&self.domain) as usize,
                start_ts: transaction.StartTS(),
                current_sql_digest: String::new(),
                state: "Idle".to_owned(),
                waiting_start_time: None,
                mem_buffer_keys: 0,
                mem_buffer_bytes: 0,
                session_id: self.connection_id(),
                database: state.current_database.clone(),
                all_sql_digests: vec![digest],
            });
        info.start_ts = transaction.StartTS();
        info.current_sql_digest.clear();
        info.state = "Idle".to_owned();
        info.waiting_start_time = None;
        info.mem_buffer_keys = mem_buffer_keys;
        info.mem_buffer_bytes = mem_buffer_bytes;
        info.session_id = self.connection_id();
        info.database = state.current_database.clone();
        Ok(())
    }

    /// Refresh a session-staleness snapshot at statement start without asking
    /// the oracle for another TSO. TiDB evaluates the negative interval
    /// against NOW for every read rather than pinning the timestamp at SET.
    pub(super) fn refresh_session_stale_read_ts(&self, sql: &str) -> SessionResult<()> {
        let lowered = sql.trim_start().to_ascii_lowercase();
        if !(lowered.starts_with("select")
            || lowered.starts_with("execute")
            || lowered.starts_with("explain"))
        {
            return Ok(());
        }
        let seconds = {
            let state = self.state.borrow();
            if state.transaction_stale_read_ts.is_some()
                || state.pending_stale_read_ts.is_some()
                || state.snapshot_read_ts.is_some()
            {
                return Ok(());
            }
            let Some(seconds) = state.session_read_staleness_seconds else {
                return Ok(());
            };
            seconds
        };
        let now_seconds = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/expression/injectNow",
        )
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });
        let stale_seconds = now_seconds.saturating_sub(seconds);
        if let Some(expected) = astersql_testkit_testfailpoint::eval_string(
            "github.com/pingcap/tidb/pkg/executor/assertStaleTSO",
        )
        .and_then(|value| value.parse::<u64>().ok())
            && expected != stale_seconds
        {
            return Err(SessionError::new(format!(
                "stale TSO physical time mismatch: expected {expected}, got {stale_seconds}"
            )));
        }
        self.state.borrow_mut().session_stale_read_ts =
            Some(stale_seconds.saturating_mul(1_000) << 18);
        Ok(())
    }

    pub(super) fn set_runtime_txn_state(&self, state: &str, waiting: bool) {
        if let Some(info) = RUNTIME_TXN_INFOS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&self.row_lock_owner)
        {
            info.state = state.to_owned();
            info.waiting_start_time = waiting.then(Instant::now);
        }
    }

    pub(super) fn acquire_row_lock_mode(
        &self,
        key: RuntimeRowLockKey,
        requested_mode: RuntimeRowLockMode,
        no_wait: bool,
        wait_timeout: Option<Duration>,
        enforce_max_execution_time: bool,
    ) -> SessionResult<()> {
        if self.state.borrow().transaction_pessimistic
            && self.domain.storage().with_storage(|store| store.Name()) != "TiKV"
        {
            let start_ts = self
                .state
                .borrow()
                .transaction
                .as_ref()
                .map_or(0, |txn| txn.StartTS());
            if let Some(error) =
                astersql_store_mockstore_unistore_tikv::server::injected_pessimistic_deadlock(
                    Some(&key.key),
                    start_ts,
                )
            {
                return Err(SessionError::with_source(
                    "[tikv:1213]Deadlock found when trying to get lock; try restarting transaction",
                    astersql_errors::SharedError::new(error),
                ));
            }
        }
        let inject_before_lock = {
            let mut session = self.state.borrow_mut();
            let inject = !session.pessimistic_pause_observed_statement;
            session.pessimistic_pause_observed_statement |= inject;
            inject
        };
        if inject_before_lock {
            self.set_runtime_txn_state("LockWaiting", true);
            astersql_testkit_testfailpoint::inject("tikvclient/beforePessimisticLock");
            self.set_runtime_txn_state("Running", false);
        }
        if self.state.borrow().transaction.is_some()
            && self.domain.storage().with_storage(|store| store.Name()) == "TiKV"
        {
            // Keep waiting on TiKV's lock manager so DATA_LOCK_WAITS observes
            // the actual RPC waiter, rather than a process-local substitute.
            let wait_ms = if no_wait {
                -1
            } else {
                let state = self.state.borrow();
                let budget = wait_timeout
                    .unwrap_or_else(|| Duration::from_secs(state.innodb_lock_wait_timeout_secs));
                let budget = if enforce_max_execution_time && state.max_execution_time_ms != 0 {
                    budget.min(Duration::from_millis(state.max_execution_time_ms))
                } else {
                    budget
                };
                budget.as_millis().min(i64::MAX as u128) as i64
            };
            self.set_runtime_txn_state("LockWaiting", true);
            let mut lock_ctx = kv::LockCtx {
                WaitTimeoutMs: wait_ms,
                Shared: requested_mode == RuntimeRowLockMode::Shared,
                ..Default::default()
            };
            let result = self
                .state
                .borrow_mut()
                .transaction
                .as_mut()
                .expect("active locking transaction")
                .LockKeys(
                    &kv::Context::default(),
                    &mut lock_ctx,
                    &[kv::Key(key.key.clone())],
                );
            self.set_runtime_txn_state("Running", false);
            result.map_err(|error| session_error("acquire TiKV row lock", error))?;
            if !lock_ctx.Shared {
                self.session_vars
                    .StmtCtx
                    .SyncExecDetails
                    .MergeLockKeysExecDetails(Some(
                        astersql_util_execdetails::execdetails::util::LockKeysDetails {
                            AggressiveLockNewCount: lock_ctx.AggressiveLockNewCount,
                            AggressiveLockDerivedCount: lock_ctx.AggressiveLockDerivedCount,
                            LockedWithConflictCount: lock_ctx.LockedWithConflictCount,
                            ..Default::default()
                        },
                    ));
            }
            self.state.borrow_mut().held_row_locks.insert(key);
            return Ok(());
        }
        let owner = self.row_lock_owner;
        let (lock_timeout, max_execution) = {
            let state = self.state.borrow();
            (
                wait_timeout
                    .unwrap_or_else(|| Duration::from_secs(state.innodb_lock_wait_timeout_secs)),
                (enforce_max_execution_time && state.max_execution_time_ms != 0)
                    .then(|| Duration::from_millis(state.max_execution_time_ms)),
            )
        };
        let started = Instant::now();
        let deadline = max_execution
            .map(|max_execution| started + max_execution.min(lock_timeout))
            .unwrap_or(started + lock_timeout);
        let (mutex, available) = &*RUNTIME_ROW_LOCKS;
        let mut state = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .connections
            .insert(owner, self.connection_id.load(Ordering::Acquire));
        loop {
            let holders = state.holders.get(&key);
            let owned_mode = holders.and_then(|holders| holders.get(&owner)).copied();
            let incompatible_holder = holders.and_then(|holders| {
                holders.iter().find_map(|(holder, mode)| {
                    if *holder == owner
                        || (requested_mode == RuntimeRowLockMode::Shared
                            && *mode == RuntimeRowLockMode::Shared)
                    {
                        None
                    } else {
                        Some(*holder)
                    }
                })
            });
            let first_waiter = state
                .waiters
                .get(&key)
                .and_then(|queue| queue.front())
                .copied();
            let queue_allows = first_waiter.is_none_or(|waiter| waiter.owner == owner);
            if incompatible_holder.is_none() && queue_allows {
                remove_runtime_waiter(&mut state, &key, owner);
                state
                    .holders
                    .entry(key.clone())
                    .or_default()
                    .insert(owner, requested_mode);
                drop(state);
                let mut session = self.state.borrow_mut();
                session.held_row_locks.insert(key);
                session
                    .pessimistic_lock_started
                    .get_or_insert_with(Instant::now);
                self.set_runtime_txn_state("Running", false);
                return Ok(());
            }
            if matches!(owned_mode, Some(RuntimeRowLockMode::Exclusive))
                || (owned_mode == Some(RuntimeRowLockMode::Shared)
                    && requested_mode == RuntimeRowLockMode::Shared)
            {
                remove_runtime_waiter(&mut state, &key, owner);
                return Ok(());
            }
            if no_wait {
                remove_runtime_waiter(&mut state, &key, owner);
                return Err(SessionError::new(
                    "[tikv:3572]Statement could not be acquired immediately and NOWAIT is set",
                ));
            }
            let holder = incompatible_holder
                .or_else(|| first_waiter.map(|waiter| waiter.owner))
                .expect("a busy lock has an owner");
            state
                .waiters
                .entry(key.clone())
                .or_default()
                .iter()
                .all(|candidate| candidate.owner != owner)
                .then(|| {
                    state
                        .waiters
                        .entry(key.clone())
                        .or_default()
                        .push_back(RuntimeRowLockWaiter {
                            owner,
                            mode: requested_mode,
                        });
                });
            state.wait_for.insert(owner, holder);
            self.set_runtime_txn_state("LockWaiting", true);
            if runtime_lock_cycle(&state, owner, holder) {
                record_runtime_deadlock(&state, owner);
                remove_runtime_waiter(&mut state, &key, owner);
                available.notify_all();
                // The deadlock victim's pessimistic transaction is aborted by
                // TiKV. Mirror that contract locally: retaining its earlier
                // row locks would leave the surviving transaction blocked
                // until the lock-wait timeout even though the cycle was
                // already resolved.
                drop(state);
                self.finish_transaction(false)?;
                return Err(SessionError::new(
                    "[tikv:1213]Deadlock found when trying to get lock; try restarting transaction",
                ));
            }
            if let Err(error) = self.sql_killer.HandleSignal() {
                remove_runtime_waiter(&mut state, &key, owner);
                available.notify_all();
                return Err(session_error("pessimistic lock interrupted", error));
            }
            let now = Instant::now();
            if now >= deadline {
                remove_runtime_waiter(&mut state, &key, owner);
                available.notify_all();
                if max_execution.is_some_and(|limit| started.elapsed() >= limit) {
                    return Err(SessionError::new(
                        "[executor:3024]Query execution was interrupted, maximum statement \
                         execution time exceeded",
                    ));
                }
                return Err(SessionError::new(
                    "[tikv:1205]Lock wait timeout exceeded; try restarting transaction",
                ));
            }
            let slice = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(10));
            let waited = available
                .wait_timeout(state, slice)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waited.0;
        }
    }

    pub(super) fn acquire_row_lock(
        &self,
        key: RuntimeRowLockKey,
        no_wait: bool,
        wait_seconds: Option<u64>,
        enforce_max_execution_time: bool,
    ) -> SessionResult<()> {
        self.acquire_row_lock_mode(
            key,
            RuntimeRowLockMode::Exclusive,
            no_wait,
            wait_seconds.map(Duration::from_secs),
            enforce_max_execution_time,
        )
    }

    pub(super) fn acquire_point_row_lock(
        &self,
        key: RuntimeRowLockKey,
        wait_ms: i64,
    ) -> SessionResult<()> {
        let no_wait = wait_ms == -1;
        let wait_timeout =
            (!no_wait && wait_ms >= 0).then(|| Duration::from_millis(wait_ms as u64));
        self.acquire_row_lock_mode(
            key,
            RuntimeRowLockMode::Exclusive,
            no_wait,
            wait_timeout,
            true,
        )
    }

    pub(super) fn acquire_row_locks(
        &self,
        mut keys: Vec<RuntimeRowLockKey>,
        no_wait: bool,
        wait_seconds: Option<u64>,
        enforce_max_execution_time: bool,
    ) -> SessionResult<()> {
        // UPDATE may already hold its record before acquiring index mutations.
        // Every batch must use that same order: otherwise DELETE can hold the
        // index while waiting on the record and manufacture a deadlock cycle.
        keys.sort_by(|left, right| {
            (!astersql_tablecodec::IsRecordKey(&left.key), &left.key)
                .cmp(&(!astersql_tablecodec::IsRecordKey(&right.key), &right.key))
        });
        keys.dedup();
        let before = self.state.borrow().held_row_locks.clone();
        for key in keys {
            if let Err(error) =
                self.acquire_row_lock(key, no_wait, wait_seconds, enforce_max_execution_time)
            {
                let acquired = self
                    .state
                    .borrow()
                    .held_row_locks
                    .difference(&before)
                    .cloned()
                    .collect::<HashSet<_>>();
                release_runtime_row_locks(self.row_lock_owner, Some(&acquired));
                self.state
                    .borrow_mut()
                    .held_row_locks
                    .retain(|key| !acquired.contains(key));
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn acquire_shared_row_locks(
        &self,
        mut keys: Vec<RuntimeRowLockKey>,
    ) -> SessionResult<()> {
        keys.sort_by(|left, right| left.key.cmp(&right.key));
        keys.dedup();
        let before = self.state.borrow().held_row_locks.clone();
        for key in keys {
            if let Err(error) =
                self.acquire_row_lock_mode(key, RuntimeRowLockMode::Shared, false, None, false)
            {
                let acquired = self
                    .state
                    .borrow()
                    .held_row_locks
                    .difference(&before)
                    .cloned()
                    .collect::<HashSet<_>>();
                release_runtime_row_locks(self.row_lock_owner, Some(&acquired));
                self.state
                    .borrow_mut()
                    .held_row_locks
                    .retain(|key| !acquired.contains(key));
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn release_all_row_locks(&self) {
        release_runtime_row_locks(self.row_lock_owner, None);
        let mut state = self.state.borrow_mut();
        state.held_row_locks.clear();
        state.pessimistic_lock_started = None;
        state.pessimistic_lock_ttl_expired_for_test = false;
    }

    pub(super) fn transaction_schema_changed(
        &self,
        start_schema: &SchemaRef,
        related_table_ids: &HashSet<i64>,
        transaction_write_keys: &HashSet<RuntimeRowLockKey>,
        locking_table_ids: &HashSet<i64>,
    ) -> SessionResult<bool> {
        let latest_schema = self.domain.info_schema();
        if start_schema.SchemaMetaVersion() == latest_schema.SchemaMetaVersion()
            || related_table_ids.is_empty()
        {
            return Ok(false);
        }
        for table_id in related_table_ids {
            // Go retries ErrInfoSchemaChanged by rebuilding and replaying the
            // transaction history.  The canonical Rust runtime keeps the
            // already-staged KV mutations instead; a table read without locking
            // or targeted by an empty UPDATE/DELETE therefore needs no replay.
            // Keep rejecting real writes through a stale table definition.
            let table_has_writes = transaction_write_keys.iter().any(|write_key| {
                astersql_tablecodec::DecodeTableID(astersql_tablecodec::kv::Key(
                    write_key.key.clone(),
                )) == *table_id
            });
            // Go includes tables accessed by locking reads in the commit schema check.
            if !table_has_writes && !locking_table_ids.contains(table_id) {
                continue;
            }
            let start = start_schema
                .TableByID(*table_id)
                .map(|table| {
                    table
                        .ModelMeta()
                        .map_err(|error| session_error("read transaction table schema", error))
                })
                .transpose()?;
            let latest = latest_schema
                .TableByID(*table_id)
                .map(|table| {
                    table
                        .ModelMeta()
                        .map_err(|error| session_error("read latest table schema", error))
                })
                .transpose()?;
            let changed = match (start, latest) {
                (Some(start), Some(latest)) => {
                    serde_json::to_vec(start.as_ref())
                        .map_err(|error| session_error("encode transaction table schema", error))?
                        != serde_json::to_vec(latest.as_ref())
                            .map_err(|error| session_error("encode latest table schema", error))?
                }
                (None, None) => false,
                _ => true,
            };
            if changed {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn can_bypass_restricted_read_only(&self, in_restricted_sql: bool) -> bool {
        if in_restricted_sql {
            return true;
        }
        let (Some(user), Some(host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) else {
            return false;
        };
        runtime_privilege_handle(&self.domain)
            .Get()
            .HasExplicitlyGrantedDynamicPrivilege(
                &self.active_roles.borrow(),
                user,
                host,
                "RESTRICTED_REPLICA_WRITER_ADMIN",
                false,
            )
    }

    pub(super) fn finish_transaction(&self, commit: bool) -> SessionResult<()> {
        self.finish_transaction_with_retry(commit, true)
    }

    /// A DDL worker retries by reloading the Job and rechecking its owner. Raw
    /// mutation replay could overwrite an administrative pause or commit after
    /// lease loss, so its transaction must propagate conflicts to the scheduler.
    pub(super) fn commit_ddl_transaction(&self) -> SessionResult<()> {
        self.finish_transaction_with_retry(true, false)
    }

    fn finish_transaction_with_retry(
        &self,
        commit: bool,
        allow_mutation_retry: bool,
    ) -> SessionResult<()> {
        struct ReleaseMDL<'a>(&'a ConcreteSession);
        impl Drop for ReleaseMDL<'_> {
            fn drop(&mut self) {
                self.0.transaction_mdl.clear();
                self.0.mdl_tables.borrow_mut().clear();
                self.0.mdl_databases.borrow_mut().clear();
                self.0.mdl_metadata_error.borrow_mut().take();
            }
        }

        let in_restricted_sql = self.state.borrow().in_restricted_sql;
        // Go checks restricted/super read-only again when committing.  Planning
        // may have happened before an administrator enabled the switch while a
        // long-running statement was executing, so checking only at dispatch
        // time would let that write escape.
        if commit
            && runtime_read_only_mode_enabled()
            && !self.can_bypass_restricted_read_only(self.state.borrow().in_restricted_sql)
            && self.state.borrow().transaction.is_some()
            && !self.state.borrow().transaction_write_keys.is_empty()
        {
            return Err(runtime_read_only_mode_error());
        }
        let _release_mdl = ReleaseMDL(self);
        if self.state.borrow().transaction.is_some() {
            self.set_runtime_txn_state(if commit { "Committing" } else { "RollingBack" }, false);
            if commit {
                astersql_testkit_testfailpoint::inject("tikvclient/beforePrewrite");
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/session/mockSlowCommit",
                );
            } else {
                astersql_testkit_testfailpoint::inject(
                    "github.com/pingcap/tidb/pkg/session/mockSlowRollback",
                );
            }
        }
        let (
            transaction,
            had_transaction,
            was_pessimistic,
            injected_commit_error,
            need_min_commit_ts,
            optimistic_fk_check_keys,
            transaction_read_epoch,
            transaction_write_keys,
            deferred_optimistic_constraint_errors,
            optimistic_for_update_keys,
            transaction_conflict_context,
            constraint_check_in_place,
            was_stale,
            global_temporary_tables,
            pending_ttl_insert_rows,
            transaction_info_schema,
            transaction_related_table_ids,
            transaction_locking_table_ids,
        ) = {
            let mut state = self.state.borrow_mut();
            let transaction = state.transaction.take();
            let had_transaction = transaction.is_some();
            let was_pessimistic = state.transaction_pessimistic;
            let injected_commit_error = (commit && had_transaction)
                .then(|| state.next_dml_commit_error.take())
                .flatten();
            let need_min_commit_ts = commit
                && had_transaction
                && state.guarantee_linearizability
                && (state.enable_async_commit || state.enable_1pc);
            let was_stale = state.transaction_stale_read_ts.is_some();
            state.transaction_pessimistic = false;
            state.transaction_explicit_optimistic = false;
            state.transaction_stale_read_ts = None;
            if let Some(isolation) = state.transaction_isolation_restore.take() {
                state.transaction_isolation = isolation;
            }
            let pending_ttl_insert_rows = std::mem::take(&mut state.pending_ttl_insert_rows);
            state.savepoints.clear();
            let optimistic_fk_check_keys = std::mem::take(&mut state.optimistic_fk_check_keys);
            let transaction_read_epoch = state.transaction_read_epoch;
            let transaction_write_keys = std::mem::take(&mut state.transaction_write_keys);
            let deferred_optimistic_constraint_errors =
                std::mem::take(&mut state.deferred_optimistic_constraint_errors);
            let optimistic_for_update_keys = std::mem::take(&mut state.optimistic_for_update_keys);
            let transaction_conflict_context = state.transaction_conflict_context.take();
            let constraint_check_in_place = state.constraint_check_in_place_pessimistic;
            let global_temporary_tables =
                std::mem::take(&mut state.global_temporary_tables_in_transaction);
            let transaction_info_schema = state.transaction_info_schema.take();
            let transaction_related_table_ids =
                std::mem::take(&mut state.transaction_related_table_ids);
            let transaction_locking_table_ids =
                std::mem::take(&mut state.transaction_locking_table_ids);
            if !commit {
                state.pending_stats_deltas.clear();
            }
            (
                transaction,
                had_transaction,
                was_pessimistic,
                injected_commit_error,
                need_min_commit_ts,
                optimistic_fk_check_keys,
                transaction_read_epoch,
                transaction_write_keys,
                deferred_optimistic_constraint_errors,
                optimistic_for_update_keys,
                transaction_conflict_context,
                constraint_check_in_place,
                was_stale,
                global_temporary_tables,
                pending_ttl_insert_rows,
                transaction_info_schema,
                transaction_related_table_ids,
                transaction_locking_table_ids,
            )
        };
        if need_min_commit_ts {
            // The transaction has already been detached from session state.
            // Releasing row locks here also makes a failpoint panic unwind
            // cleanly instead of leaking ownership into later transactions.
            self.release_all_row_locks();
            if astersql_testkit_testfailpoint::is_active("tikvclient/getMinCommitTSFromTSO") {
                astersql_testkit_testfailpoint::inject("tikvclient/getMinCommitTSFromTSO");
            }
        }
        let mut committed_store_ts = None;
        let mut result = (|| -> SessionResult<()> {
            let Some(mut transaction) = transaction else {
                return Ok(());
            };
            if commit {
                let schema_checker = if !transaction.IsReadOnly() {
                    self.schema_validator
                        .borrow()
                        .as_ref()
                        .cloned()
                        .zip(transaction_info_schema.as_ref())
                        .map(|(validator, schema)| {
                            let mut tables = transaction_write_keys
                                .iter()
                                .map(|key| {
                                    astersql_tablecodec::DecodeTableID(
                                        astersql_tablecodec::kv::Key(key.key.clone()),
                                    )
                                })
                                .chain(transaction_locking_table_ids.iter().copied())
                                .filter(|id| *id != 0)
                                .filter(|id| {
                                    schema
                                        .TableByID(*id)
                                        .and_then(|table| table.ModelMeta().ok())
                                        .is_none_or(|table| {
                                            table.TempTableType
                                                == astersql_meta_model::TempTableNone
                                        })
                                })
                                .collect::<Vec<_>>();
                            tables.sort_unstable();
                            tables.dedup();
                            super::schema_validation::checker(
                                validator,
                                schema.SchemaMetaVersion(),
                                tables,
                                !self.session_vars.TxnCtx.noNeedToRestore.EnableMDL,
                            )
                        })
                } else {
                    None
                };
                if let Some(checker) = schema_checker.clone() {
                    transaction.SetOption(kv::SchemaChecker, Some(Box::new(checker)));
                }
                if was_stale && transaction_write_keys.is_empty() {
                    transaction.Rollback().map_err(|error| {
                        session_error("close read-only stale transaction", error)
                    })?;
                    return Ok(());
                }
                if astersql_testkit_testfailpoint::is_active(
                    "github.com/pingcap/tidb/pkg/session/mockCommitError8942",
                ) {
                    astersql_testkit_testfailpoint::inject(
                        "github.com/pingcap/tidb/pkg/session/mockCommitError8942",
                    );
                    transaction.Rollback().map_err(|error| {
                        session_error("rollback mock statement commit failure", error)
                    })?;
                    return Err(SessionError::new(
                        "[session:8942]mock statement commit error",
                    ));
                }
                if let Some(message) = injected_commit_error {
                    transaction.Rollback().map_err(|error| {
                        session_error("rollback injected commit failure", error)
                    })?;
                    return Err(SessionError::new(message));
                }
                let schema_changed = if was_stale {
                    false
                } else if let Some(start_schema) = transaction_info_schema.as_ref() {
                    match self.transaction_schema_changed(
                        start_schema,
                        &transaction_related_table_ids,
                        &transaction_write_keys,
                        &transaction_locking_table_ids,
                    ) {
                        Ok(changed) => changed,
                        Err(error) => {
                            transaction.Rollback().map_err(|rollback_error| {
                                session_error(
                                    "rollback transaction after schema-check failure",
                                    rollback_error,
                                )
                            })?;
                            return Err(error);
                        }
                    }
                } else {
                    false
                };
                if schema_changed {
                    transaction.Rollback().map_err(|error| {
                        session_error("rollback transaction with changed schema", error)
                    })?;
                    return Err(SessionError::new(
                        "[domain:8028]Information schema is changed during the execution of the \
                         statement(for example, table definition may be updated by other DDL ran \
                         in parallel). If you see this error often, try increasing \
                         `tidb_max_delta_schema_count`. [try again later]",
                    ));
                }
                for (key, message) in &deferred_optimistic_constraint_errors {
                    match transaction.Get(&kv::Context::default(), kv::Key(key.clone()), &[]) {
                        Ok(_) => {
                            transaction.Rollback().map_err(|error| {
                                session_error("rollback deferred constraint violation", error)
                            })?;
                            return Err(SessionError::new(message.clone()));
                        }
                        Err(error) if kv::IsErrNotFound(&error) => {}
                        Err(error) => {
                            let _ = transaction.Rollback();
                            return Err(session_error(
                                "read deferred constraint key before commit",
                                error,
                            ));
                        }
                    }
                }
                let latest = self
                    .domain
                    .storage()
                    .with_storage(
                        |store| -> Result<Box<dyn kv::Snapshot>, kv::errors::SharedError> {
                            let version = store.CurrentVersion("global")?;
                            Ok(store.GetSnapshot(version))
                        },
                    )
                    .map_err(|error| session_error("open commit conflict snapshot", error))?;
                let runtime_conflict = (!was_pessimistic || !constraint_check_in_place) && {
                    let epochs = RUNTIME_KEY_COMMIT_EPOCHS
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    transaction_write_keys.iter().any(|key| {
                        epochs
                            .get(key)
                            .is_some_and(|epoch| *epoch > transaction_read_epoch)
                    })
                };
                let mut conflict_keys = transaction_write_keys
                    .iter()
                    .map(|key| key.key.clone())
                    .chain(optimistic_fk_check_keys.iter().cloned())
                    .collect::<Vec<_>>();
                conflict_keys.sort();
                conflict_keys.dedup();
                // A real TiKV transaction must reach prewrite: its MVCC conflict
                // and rollback-record checks are authoritative, including equal-TS collisions.
                let uses_tikv = self.domain.storage().with_storage(|store| store.Name()) == "TiKV";
                let conflict = !uses_tikv
                    && (runtime_conflict
                        || (!was_pessimistic
                            && Self::snapshots_differ_on_keys(
                                transaction.GetSnapshot(),
                                latest.as_ref(),
                                &conflict_keys,
                            )?));
                if conflict {
                    if in_restricted_sql && allow_mutation_retry {
                        let mutations = transaction_write_keys
                            .iter()
                            .map(|key| {
                                let storage_key = kv::Key(key.key.clone());
                                match transaction.Get(&kv::Context::default(), storage_key, &[]) {
                                    Ok(value) => Ok((key.key.clone(), Some(value.Value))),
                                    Err(error) if kv::IsErrNotFound(&error) => {
                                        Ok((key.key.clone(), None))
                                    }
                                    Err(error) => Err(session_error(
                                        "read restricted transaction mutation for retry",
                                        error,
                                    )),
                                }
                            })
                            .collect::<SessionResult<Vec<_>>>()?;
                        transaction.Rollback().map_err(|error| {
                            session_error("rollback restricted transaction before retry", error)
                        })?;
                        let mut retry = self
                            .domain
                            .storage()
                            .with_storage(|store| store.Begin(&[]))
                            .map_err(|error| {
                                session_error("begin restricted transaction retry", error)
                            })?;
                        for (key, value) in mutations {
                            match value {
                                Some(value) => retry.Set(kv::Key(key), value),
                                None => retry.Delete(kv::Key(key)),
                            }
                            .map_err(|error| {
                                session_error("apply restricted transaction retry", error)
                            })?;
                        }
                        if let Some(checker) = schema_checker {
                            retry.SetOption(kv::SchemaChecker, Some(Box::new(checker)));
                        }
                        retry.Commit(&kv::Context::default()).map_err(|error| {
                            session_error("commit restricted transaction retry", error)
                        })?;
                        committed_store_ts = Some(retry.CommitTS());
                        Ok(())
                    } else {
                        let _ = transaction.Rollback();
                        let detail = transaction_conflict_context
                        .as_ref()
                        .map(|(table, handle)| {
                            format!(
                                ", conflict={{tableName={}.{}, handle={handle}}}, reason=Optimistic",
                                self.current_database(),
                                table
                            )
                        })
                        .unwrap_or_else(|| ", reason=Optimistic".to_owned());
                        Err(SessionError::new(format!(
                            "[kv:9007]Write conflict, transaction commit failed{detail} {}",
                            astersql_kv::TxnRetryableMark
                        )))
                    }
                } else {
                    transaction
                        .Commit(&kv::Context::default())
                        .map_err(|error| {
                            if kv::ErrWriteConflict.Equal(Some(&error)) {
                                let detail = transaction_conflict_context
                                    .as_ref()
                                    .map(|(table, handle)| {
                                        format!(
                                            ", conflict={{tableName={}.{}, handle={handle}}}",
                                            self.current_database(),
                                            table
                                        )
                                    })
                                    .unwrap_or_default();
                                SessionError::new(format!("{error}{detail}"))
                            } else {
                                session_error("commit transaction", error)
                            }
                        })?;
                    committed_store_ts = Some(transaction.CommitTS());
                    Ok(())
                }
            } else {
                transaction
                    .Rollback()
                    .map_err(|error| session_error("rollback transaction", error))
            }
        })();
        self.release_all_row_locks();
        if commit && had_transaction && result.is_ok() {
            for table in global_temporary_tables.values() {
                if let Err(error) = self.clear_temporary_table_data(table, "OnCommitDeleteRows") {
                    result = Err(error);
                    break;
                }
            }
        }
        if commit && had_transaction && result.is_ok() {
            let epoch = NEXT_RUNTIME_COMMIT_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
            let mut epochs = RUNTIME_KEY_COMMIT_EPOCHS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for key in transaction_write_keys
                .iter()
                .chain(optimistic_for_update_keys.iter())
            {
                epochs.insert(key.clone(), epoch);
            }
            let store_commit_ts = committed_store_ts.unwrap_or(epoch);
            let commit_ts = astersql_testkit_testfailpoint::eval_string(
                "github.com/pingcap/tidb/pkg/session/mockFutureCommitTS",
            )
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(store_commit_ts);
            let mut state = self.state.borrow_mut();
            state.last_commit_ts = commit_ts;
            if committed_store_ts.is_some() {
                state.last_observed_store_ts = store_commit_ts;
                state.tso_catalog_versions.insert(
                    store_commit_ts,
                    self.domain.stats_context().catalog_version(),
                );
                state
                    .tso_wall_times
                    .push((SystemTime::now(), store_commit_ts));
            }
            drop(state);
            self.flush_pending_stats_deltas();
        }
        if commit && had_transaction {
            increment_ttl_insert_rows_metric(pending_ttl_insert_rows);
        }
        result
    }

    pub(super) fn begin_transaction(&self, statement: &ast::BeginStmt) -> SessionResult<()> {
        if statement.ReadOnly
            && statement.AsOf.is_none()
            && !self.state.borrow().enable_noop_functions
        {
            return Err(SessionError::new(
                "read only transaction is only supported when \
                 tidb_enable_noop_functions is enabled",
            ));
        }
        let stale_read_ts = if let Some(as_of) = statement.AsOf.as_ref() {
            if self.state.borrow().pending_stale_read_ts.is_some() {
                return Err(SessionError::new(
                    "start transaction as of is forbidden after set transaction read only as of",
                ));
            }
            Some(self.evaluate_stale_read_ts(&as_of.TsExpr)?)
        } else {
            self.state.borrow_mut().pending_stale_read_ts.take()
        };
        if self.state.borrow().transaction.is_some() {
            self.finish_transaction(true)?;
        }
        // A previous autocommit statement must not lend its process-local
        // pessimistic locks to the next explicit transaction. Keep this at
        // the transaction boundary so the session state and global lock table
        // are reset together before the new StartTS is installed.
        self.release_all_row_locks();
        let mode = {
            let state = self.state.borrow();
            if statement.Mode.is_empty() {
                state.txn_mode.clone()
            } else {
                statement.Mode.clone()
            }
        };
        let pessimistic = mode.eq_ignore_ascii_case("pessimistic");
        if stale_read_ts.is_some() {
            let _ = astersql_testkit_testfailpoint::eval_string("tikvclient/injectTxnScope");
        }
        let mut transaction = self
            .domain
            .storage()
            .with_storage(|store| {
                if let Some(read_ts) = stale_read_ts {
                    store.Begin(&[kv::tikv::TxnOption::StartTS(read_ts)])
                } else {
                    store.Begin(&[])
                }
            })
            .map_err(|error| session_error("begin transaction", error))?;
        let transaction_store_labels = astersql_config::get_global_config().labels.clone();
        let transaction_scope = runtime_txn_scope(&transaction_store_labels);
        let transaction_info_schema = self.domain.info_schema();
        let mut state = self.state.borrow_mut();
        transaction.SetOption(
            kv::EnableAsyncCommit,
            Some(Box::new(state.enable_async_commit)),
        );
        transaction.SetOption(kv::Pessimistic, Some(Box::new(pessimistic)));
        transaction.SetOption(kv::Enable1PC, Some(Box::new(state.enable_1pc)));
        transaction.SetOption(kv::TxnScope, Some(Box::new(transaction_scope.clone())));
        transaction.SetOption(
            kv::MatchStoreLabels,
            Some(Box::new(transaction_store_labels.clone())),
        );
        if let Some(isolation) = state.transaction_isolation_one_shot.take() {
            state.transaction_isolation_restore = Some(state.transaction_isolation.clone());
            state.transaction_isolation = isolation;
        }
        self.session_vars
            .TxnCtx
            .FairLockingUsed
            .store(false, std::sync::atomic::Ordering::Release);
        self.session_vars
            .TxnCtx
            .FairLockingEffective
            .store(false, std::sync::atomic::Ordering::Release);
        state.transaction = Some(transaction);
        state.transaction_info_schema = Some(transaction_info_schema);
        self.transaction_mdl.clear();
        self.mdl_tables.borrow_mut().clear();
        self.mdl_databases.borrow_mut().clear();
        self.mdl_metadata_error.borrow_mut().take();
        state.transaction_related_table_ids.clear();
        state.transaction_locking_table_ids.clear();
        state.statement_txn_start_ts = state
            .transaction
            .as_ref()
            .map_or(0, |transaction| transaction.StartTS());
        state.transaction_pessimistic = pessimistic;
        state.transaction_explicit_optimistic = mode.eq_ignore_ascii_case("optimistic");
        state.transaction_stale_read_ts = stale_read_ts;
        if stale_read_ts.is_some() {
            state.snapshot_catalog_version = None;
            state.snapshot_read_ts = None;
        }
        state.savepoints.clear();
        state.pending_ttl_insert_rows = 0;
        state.optimistic_fk_check_keys.clear();
        state.txn_mem_buffer_keys = 0;
        state.txn_mem_buffer_bytes = 0;
        state.transaction_read_epoch = NEXT_RUNTIME_COMMIT_EPOCH.load(Ordering::Acquire);
        state.transaction_write_keys.clear();
        state.deferred_optimistic_constraint_errors.clear();
        state.optimistic_for_update_keys.clear();
        state.transaction_conflict_context = None;
        state.transaction_scope = transaction_scope;
        state.transaction_store_labels = transaction_store_labels;
        Ok(())
    }

    /// Start the implicit transaction owned by `autocommit=0` at its first DML.
    pub(super) fn ensure_implicit_transaction(&self) -> SessionResult<()> {
        let should_begin = {
            let state = self.state.borrow();
            !state.autocommit && state.transaction.is_none()
        };
        if should_begin {
            self.begin_transaction(&ast::BeginStmt::default())?;
        }
        Ok(())
    }

    pub(super) fn create_savepoint(&self, statement: &ast::SavepointStmt) -> SessionResult<()> {
        let mut state = self.state.borrow_mut();
        let Some(transaction) = state.transaction.as_ref() else {
            return Ok(());
        };
        if state.transaction_pessimistic && !state.constraint_check_in_place_pessimistic {
            return Err(SessionError::new(
                "savepoint is not supported in pessimistic transactions when in-place \
                 constraint check is disabled",
            ));
        }
        let visible = Self::transaction_visible(transaction.as_ref())?;
        let name = statement.Name.to_ascii_lowercase();
        state.savepoints.retain(|savepoint| savepoint.name != name);
        let held_locks = state.held_row_locks.clone();
        let deferred_optimistic_constraint_errors =
            state.deferred_optimistic_constraint_errors.clone();
        let pending_ttl_insert_rows = state.pending_ttl_insert_rows;
        state.savepoints.push(RuntimeSavepoint {
            name,
            visible,
            held_locks,
            deferred_optimistic_constraint_errors,
            pending_ttl_insert_rows,
        });
        Ok(())
    }

    pub(super) fn release_savepoint(
        &self,
        statement: &ast::ReleaseSavepointStmt,
    ) -> SessionResult<()> {
        let name = statement.Name.to_ascii_lowercase();
        let mut state = self.state.borrow_mut();
        let Some(position) = state
            .savepoints
            .iter()
            .rposition(|savepoint| savepoint.name == name)
        else {
            return Err(SessionError::new(format!(
                "[executor:1305]SAVEPOINT {} does not exist",
                statement.Name
            )));
        };
        state.savepoints.remove(position);
        Ok(())
    }

    pub(super) fn rollback_to_savepoint(&self, name: &str) -> SessionResult<()> {
        let name_lower = name.to_ascii_lowercase();
        let (visible, retained_locks, released_locks) = {
            let mut state = self.state.borrow_mut();
            let Some(position) = state
                .savepoints
                .iter()
                .rposition(|savepoint| savepoint.name == name_lower)
            else {
                return Err(SessionError::new(format!(
                    "[executor:1305]SAVEPOINT {name} does not exist"
                )));
            };
            let savepoint = state.savepoints[position].clone();
            let transaction = state
                .transaction
                .as_mut()
                .ok_or_else(|| SessionError::new("no active transaction"))?;
            let current = Self::transaction_visible(transaction.as_ref())?;
            for key in current.keys() {
                if !savepoint.visible.contains_key(key) {
                    transaction
                        .Delete(kv::Key(key.clone()))
                        .map_err(|error| session_error("rollback savepoint delete", error))?;
                }
            }
            for (key, value) in &savepoint.visible {
                if current.get(key) != Some(value) {
                    transaction
                        .Set(kv::Key(key.clone()), value.clone())
                        .map_err(|error| session_error("rollback savepoint restore", error))?;
                }
            }
            let released = state
                .held_row_locks
                .difference(&savepoint.held_locks)
                .cloned()
                .collect::<HashSet<_>>();
            state.held_row_locks = savepoint.held_locks.clone();
            state.deferred_optimistic_constraint_errors =
                savepoint.deferred_optimistic_constraint_errors.clone();
            state.pending_ttl_insert_rows = savepoint.pending_ttl_insert_rows;
            state.savepoints.truncate(position + 1);
            (savepoint.visible, savepoint.held_locks, released)
        };
        let _ = (visible, retained_locks);
        release_runtime_row_locks(self.row_lock_owner, Some(&released_locks));
        Ok(())
    }

    /// Go `ResetContextOfStmt` for a DML statement: the value-conversion
    /// strictness follows `sql_mode`.
    /// DML 值转换严格性跟随 sql_mode（对应 Go ResetContextOfStmt）。
    pub(super) fn dml_type_flags(&self) -> astersql_types::Flags {
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .unwrap_or(astersql_parser_mysql::r#const::SQLMode(0));
        let strict = mode.HasStrictMode();
        astersql_types::scalar::StrictFlags
            .WithTruncateAsWarning(!strict)
            .WithIgnoreInvalidDateErr(mode.HasAllowInvalidDatesMode())
            .WithIgnoreZeroInDate(
                !mode.HasNoZeroInDateMode() || !strict || mode.HasAllowInvalidDatesMode(),
            )
            .WithIgnoreZeroDateErr(!mode.HasNoZeroDateMode() || !strict)
    }

    /// 返回绑定的 Domain。
    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }

    pub(super) fn runtime_topology(&self) -> Vec<RuntimeStoreNode> {
        RUNTIME_TOPOLOGIES
            .lock()
            .expect("runtime topology map poisoned")
            .get(&(Arc::as_ptr(&self.domain) as usize))
            .cloned()
            .unwrap_or_default()
    }

    /// 返回可共享的 SQLKiller（用于取消查询）。
    pub fn SQLKiller(&self) -> Arc<SQLKiller> {
        Arc::clone(&self.sql_killer)
    }

    pub fn BeginProtocolResponse(&self) {
        self.state.borrow_mut().defer_protocol_finish = true;
    }

    /// Complete the protocol-visible statement after its response has been written.
    pub fn FinishProtocolResponse(&self, write_duration: Duration) {
        let pending = {
            let mut state = self.state.borrow_mut();
            state.last_write_sql_resp_duration = write_duration;
            state.defer_protocol_finish = false;
            state.pending_protocol_slow_logs.pop_front()
        };
        if let Some((force_slow_log, mut slow_log_items)) = pending {
            slow_log_items.WriteSQLRespTotal = write_duration;
            astersql_executor::adapter_slow_log::WriteForcedSlowLog(
                force_slow_log,
                &self.session_vars,
                &slow_log_items,
            );
        }
    }

    /// Read-only protocol response accounting, also available to server crate regressions.
    pub fn LastWriteSQLRespDurationForTest(&self) -> Duration {
        self.state.borrow().last_write_sql_resp_duration
    }

    /// Resolve original engine result types without executing or registering a
    /// statement. This uses the same AST/catalog resolver as prepared metadata.
    pub fn describe_result_fields(&self, sql: &str) -> SessionResult<Vec<ConcreteResultField>> {
        let statements = parse(sql)?;
        if statements.len() != 1 {
            return Err(SessionError::new("result metadata requires one statement"));
        }
        let statement = &statements[0];
        if statement.as_any().is::<ast::SelectStmt>() || statement.as_any().is::<ast::SetOprStmt>()
        {
            let mut fields = self.relational_query_node_result_fields(statement.as_ref(), &[])?;
            // Preserve explicitly declared CAST types for consumers of original
            // metadata. The existing MySQL prepared metadata resolver is unchanged.
            if let Some(select) = statement.as_any().downcast_ref::<ast::SelectStmt>() {
                if select.Fields.Fields.len() == fields.len()
                    && select.Fields.Fields.iter().all(|f| f.WildCard.is_none())
                {
                    for (field, projection) in fields.iter_mut().zip(&select.Fields.Fields) {
                        let mut expression = projection.Expr.as_ref();
                        while let Some(expr) = expression {
                            match &expr.Kind {
                                ast::ExprKind::Cast { Tp, .. } => {
                                    field.column.FieldType = Tp.clone();
                                    break;
                                }
                                ast::ExprKind::Parentheses(inner)
                                | ast::ExprKind::Collate { Expr: inner, .. } => {
                                    expression = Some(inner.as_ref());
                                }
                                _ => break,
                            }
                        }
                    }
                }
            }
            Ok(fields)
        } else {
            Ok(Vec::new())
        }
    }

    /// Parse and register a binary-protocol prepared statement without
    /// executing it. SELECT metadata is derived from the canonical AST and
    /// catalog, matching the normal relational query field resolver.
    pub fn prepare_protocol_statement(
        &self,
        sql: &str,
    ) -> SessionResult<(u64, usize, Vec<ConcreteResultField>)> {
        let statements = parse(sql)?;
        if statements.len() != 1 {
            return Err(SessionError::new("prepared SQL must contain one statement"));
        }
        let statement = &statements[0];
        let fields = if statement.as_any().is::<ast::SelectStmt>()
            || statement.as_any().is::<ast::SetOprStmt>()
            || statement.as_any().is::<ast::SetOprSelectList>()
        {
            self.relational_query_node_result_fields(statement.as_ref(), &[])?
        } else {
            Vec::new()
        };
        let parameter_count = parameter_markers(sql).len();
        if let Err(limit) = runtime_prepared_stmt_reserve(&self.domain) {
            return Err(SessionError::new(format!(
                "Can't create more than maxPreparedStmtCount statements (current value: {limit})"
            )));
        }
        let mut state = self.state.borrow_mut();
        let statement_id = state.next_prepared_id;
        state.next_prepared_id += 1;
        state.prepared.insert(statement_id, sql.to_owned());
        state.protocol_prepared_planned.insert(statement_id, false);
        Ok((statement_id, parameter_count, fields))
    }

    /// Execute a registered binary-protocol statement with typed, token-aware
    /// parameter binding. Values are rendered only as validated SQL literals;
    /// marker positions inside strings/comments are never replaced.
    pub fn execute_protocol_statement(
        &self,
        statement_id: u64,
        arguments: &[ConcretePreparedArgument],
    ) -> SessionResult<Vec<ConcreteRecordSet>> {
        let generation = runtime_plan_cache_generation(&self.domain);
        let from_plan_cache = {
            let mut state = self.state.borrow_mut();
            if state.plan_cache_generation != generation {
                for prepared in state.prepared_by_name.values_mut() {
                    prepared.planned = false;
                    prepared.cached_transaction_contexts.clear();
                }
                for planned in state.protocol_prepared_planned.values_mut() {
                    *planned = false;
                }
                state.plan_cache_generation = generation;
            }
            state.prepared_plan_cache
                && state
                    .protocol_prepared_planned
                    .get(&statement_id)
                    .copied()
                    .unwrap_or(false)
                && !arguments
                    .iter()
                    .any(|argument| matches!(argument, ConcretePreparedArgument::Null))
        };
        let sql = self
            .state
            .borrow()
            .prepared
            .get(&statement_id)
            .cloned()
            .ok_or_else(|| {
                SessionError::new(format!("unknown prepared statement {statement_id}"))
            })?;
        let literals = arguments
            .iter()
            .map(ConcretePreparedArgument::sql_literal)
            .collect::<SessionResult<Vec<_>>>()?;
        let bound = bind_parameter_markers(&sql, &literals)?;
        self.state.borrow_mut().observation_sql_override = Some(sql);
        let execution = self.execute(&bound);
        let mut state = self.state.borrow_mut();
        state.observation_sql_override = None;
        state.last_plan_from_cache = from_plan_cache;
        let prepared_plan_cache = state.prepared_plan_cache;
        if execution.is_ok()
            && let Some(planned) = state.protocol_prepared_planned.get_mut(&statement_id)
        {
            *planned = prepared_plan_cache;
        }
        drop(state);
        execution
    }

    /// Remove one binary-protocol prepared statement.
    pub fn close_protocol_statement(&self, statement_id: u64) -> SessionResult<()> {
        let mut state = self.state.borrow_mut();
        state.protocol_prepared_planned.remove(&statement_id);
        state.prepared.remove(&statement_id).ok_or_else(|| {
            SessionError::new(format!("unknown prepared statement {statement_id}"))
        })?;
        runtime_prepared_stmt_release(&self.domain, 1);
        Ok(())
    }

    /// Go `SessionVars.InRestrictedSQL`: marks this session as an internal one,
    /// which makes ANALYZE record its job as an auto-analyze job.
    /// 标记为内部受限 SQL 会话，使 ANALYZE 记为自动分析任务。
    pub fn SetInRestrictedSQL(&self, restricted: bool) {
        self.state.borrow_mut().in_restricted_sql = restricted;
        self.transaction_mdl.set_restricted(restricted);
    }

    /// COM_STMT_SEND_LONG_DATA uses the existing session root tracker, including
    /// consumption from other statements. Refuse before Consume to keep this
    /// response-free protocol command from running an OOM action.
    pub fn ChargeBoundLongData(&self, bytes: i64) -> (bool, u64) {
        let (consumed, quota) = self.BoundLongDataMemorySnapshot();
        if bytes > 0 && quota > 0 && consumed + bytes >= quota {
            return (false, self.connection_id());
        }
        self.mem_tracker.borrow().Consume(bytes);
        (true, self.connection_id())
    }

    pub fn BoundLongDataMemorySnapshot(&self) -> (i64, i64) {
        let quota = self
            .session_vars
            .GetSystemVar(astersql_sessionctx_vardef::TiDBMemQuotaQuery)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(astersql_sessionctx_vardef::DefTiDBMemQuotaQuery);
        let tracker = self.mem_tracker.borrow();
        tracker.SetBytesLimit(quota);
        (tracker.BytesConsumed(), tracker.GetBytesLimit())
    }

    /// Returns the number of statement MemTracker children currently attached to
    /// the session root tracker. Mirrors Go `MemTracker.GetChildrenForTest()`.
    /// 返回挂到会话根 MemTracker 下的语句级子跟踪器数量。
    pub fn MemTrackerChildrenForTest(&self) -> usize {
        self.mem_tracker.borrow().GetChildrenForTest().len()
    }

    /// 返回会话根 MemTracker 当前记账字节数。
    pub fn MemTrackerBytesConsumedForTest(&self) -> i64 {
        self.mem_tracker.borrow().BytesConsumed()
    }

    /// 返回最近语句 tracker 的峰值消费字节数，对应 Go MaxConsumed。
    pub fn MemTrackerMaxConsumedForTest(&self) -> i64 {
        self.last_statement_tracker
            .borrow()
            .as_ref()
            .map_or(0, |tracker| tracker.MaxConsumed())
    }

    /// 返回最近语句中 CTE 临时文件的磁盘峰值字节数。
    pub fn DiskTrackerMaxConsumedForTest(&self) -> i64 {
        self.last_statement_disk_max.get()
    }

    /// 语句完成后不应残留任何 CTE 物化作用域。
    pub fn CTEStorageMapIsEmptyForTest(&self) -> bool {
        self.cte_scopes.borrow().iter().all(HashMap::is_empty)
    }

    /// 返回最近语句清理 finished action 后的 OOM fallback 优先级。
    pub fn StatementOOMActionPriorityForTest(&self) -> Option<i64> {
        let trackers = self.last_statement_tracker.borrow();
        let tracker = trackers.as_ref()?;
        let action = tracker.GetFallbackForTest(true)?;
        let priority = action.GetPriority();
        tracker.SetActionOnExceed(Some(action));
        Some(priority)
    }

    /// 以只读方式访问会话变量。
    pub fn WithSessionVars<R>(
        &self,
        inspect: impl FnOnce(&astersql_sessionctx_variable::session::SessionVars) -> R,
    ) -> R {
        inspect(&self.session_vars)
    }

    /// 返回最近一轮替代逻辑计划的解相关与同序 IndexJoin 信号。
    pub fn AlternativeLogicalPlanSignalsForTest(&self) -> (bool, bool) {
        let (decorrelated, same_order_index_join, ..) =
            self.session_vars.StmtCtx.AlternativeLogicalPlanSignals();
        (decorrelated, same_order_index_join)
    }

    pub(super) fn alternative_logical_plans_enabled(&self) -> bool {
        self.session_vars
            .GetSystemVar(astersql_sessionctx_vardef::TiDBOptEnableAlternativeLogicalPlans)
            .is_some_and(|value| variable_is_on(&value))
    }

    pub(super) fn observe_alternative_logical_plan(
        &self,
        statement: &ast::SelectStmt,
        statement_sql: &str,
    ) -> SessionResult<()> {
        self.session_vars.StmtCtx.ResetAlternativeRoundSignals();
        if !self.alternative_logical_plans_enabled() {
            return Ok(());
        }
        let projected_subquery = statement
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .any(relational_expression_has_subquery);
        let exists_subquery = statement.Where.as_ref().is_some_and(|where_clause| {
            matches!(where_clause.Kind, ast::ExprKind::ExistsSubquery { .. })
        });
        if !projected_subquery && !exists_subquery {
            return Ok(());
        }
        self.session_vars
            .StmtCtx
            .MarkAlternativeLogicalPlanDecorrelatedApply();
        let exists_subquery_has_index_join = statement
            .Where
            .as_ref()
            .and_then(|where_clause| {
                let ast::ExprKind::ExistsSubquery { Sel, Not: false } = &where_clause.Kind else {
                    return None;
                };
                let ast::ExprKind::Subquery { Query, .. } = &Sel.Kind else {
                    return None;
                };
                Query
                    .with_node(|node| {
                        let inner = node.as_any().downcast_ref::<ast::SelectStmt>()?;
                        let from = inner.From.as_ref()?;
                        if from.TableRefs.Right.is_some() {
                            return Some(false);
                        }
                        let ast::ResultSetNode::TableSource(source) =
                            from.TableRefs.Left.as_deref()?
                        else {
                            return Some(false);
                        };
                        let database = if source.Source.Schema.L.is_empty() {
                            self.current_database()
                        } else {
                            source.Source.Schema.L.clone()
                        };
                        self.mdl_stats_table(&database, &source.Source.Name.L)
                            .map(|(_, table)| !table.Indices.is_empty())
                    })
                    .flatten()
            })
            .unwrap_or(false);
        if exists_subquery_has_index_join {
            self.session_vars
                .StmtCtx
                .MarkAlternativeLogicalPlanSameOrderIndexJoin();
            return Ok(());
        }
        const FAILPOINT: &str =
            "github.com/pingcap/tidb/pkg/planner/failIfAlternativeLogicalPlanRoundTriggered";
        if astersql_testkit_testfailpoint::eval_string(FAILPOINT).is_some_and(|expected| {
            expected.trim_matches(['"', '\'']) == format!("non-decorrelate:{statement_sql}")
        }) {
            return Err(SessionError::new(
                "unexpected alternative logical plan round: non-decorrelate",
            ));
        }
        Ok(())
    }

    /// 设置会话系统变量。
    pub fn SetSessionSystemVar(&mut self, name: &str, value: &str) -> SessionResult<()> {
        let normalized = name.to_ascii_lowercase();
        match normalized.as_str() {
            "tidb_enable_async_commit" => {
                self.state.borrow_mut().enable_async_commit = variable_is_on(value);
                return Ok(());
            }
            "tidb_guarantee_linearizability" => {
                self.state.borrow_mut().guarantee_linearizability = variable_is_on(value);
                return Ok(());
            }
            "tidb_enable_1pc" => {
                self.state.borrow_mut().enable_1pc = variable_is_on(value);
                return Ok(());
            }
            "tikv_client_read_timeout" => {
                self.state.borrow_mut().tikv_client_read_timeout_ms = value
                    .trim_matches(['\'', '"'])
                    .parse::<u64>()
                    .map_err(|error| session_error("parse tikv_client_read_timeout", error))?;
                return Ok(());
            }
            "tidb_replica_read" => {
                self.state.borrow_mut().replica_read =
                    value.trim_matches(['\'', '"']).to_ascii_lowercase();
                return Ok(());
            }
            "tidb_enable_paging" => {
                self.state.borrow_mut().enable_paging = variable_is_on(value);
                return Ok(());
            }
            "tidb_min_paging_size" => {
                self.state.borrow_mut().min_paging_size = value
                    .trim_matches(['\'', '"'])
                    .parse::<usize>()
                    .map_err(|error| session_error("parse tidb_min_paging_size", error))?;
                return Ok(());
            }
            _ => {}
        }
        if name.eq_ignore_ascii_case(astersql_sessionctx_vardef::TiDBOptUseInvisibleIndexes) {
            self.state.borrow_mut().use_invisible_indexes = variable_is_on(value);
        }
        let variables = Arc::get_mut(&mut self.session_vars).ok_or_else(|| {
            SessionError::new("session variables are in use by an active planner context")
        })?;
        variables
            .SetSystemVar(name, value)
            .map_err(|error| session_error("set session system variable", error))
    }

    /// 添加会话级 SQL Binding。
    pub fn AddSessionBinding(&self, binding: astersql_bindinfo::Binding) {
        self.bindings.borrow_mut().AddSessionBinding(binding);
    }

    pub(super) fn read_session_kv(&self, key: &str) -> SessionResult<Option<Vec<u8>>> {
        let state = self.state.borrow();
        let result = if let Some(transaction) = state.transaction.as_ref() {
            transaction.Get(&kv::Context::default(), storage_key(key), &[])
        } else {
            self.domain.storage().with_storage(|store| {
                let version = store.CurrentVersion("global")?;
                store
                    .GetSnapshot(version)
                    .Get(&kv::Context::default(), storage_key(key), &[])
            })
        };
        match result {
            Ok(value) => Ok(Some(value.Value)),
            Err(error) if kv::IsErrNotFound(&error) => Ok(None),
            Err(error) => Err(session_error("read session KV for DML", error)),
        }
    }

    /// 将会话 KV 变更写入事务。
    pub(super) fn apply_session_kv_mutations(
        &self,
        operator: &str,
        mutations: Vec<(String, Option<String>)>,
        affected_rows: u64,
    ) -> SessionResult<()> {
        let write_keys = mutations.len() as u64;
        let mut state = self.state.borrow_mut();
        let mut committed = false;
        let mut commit_wait = std::time::Duration::ZERO;
        if let Some(transaction) = state.transaction.as_mut() {
            for (key, value) in mutations {
                match value {
                    Some(value) => transaction
                        .Set(storage_key(&key), value.into_bytes())
                        .map_err(|error| session_error("write transaction", error))?,
                    None => transaction
                        .Delete(storage_key(&key))
                        .map_err(|error| session_error("delete transaction", error))?,
                }
            }
        } else {
            let injected_commit_error = state.next_dml_commit_error.take();
            let mut transaction = self
                .domain
                .storage()
                .with_storage(|store| store.Begin(&[]))
                .map_err(|error| session_error("begin autocommit", error))?;
            for (key, value) in mutations {
                let result = match value {
                    Some(value) => transaction.Set(storage_key(&key), value.into_bytes()),
                    None => transaction.Delete(storage_key(&key)),
                };
                if let Err(error) = result {
                    let _ = transaction.Rollback();
                    return Err(session_error("apply autocommit DML", error));
                }
            }
            if let Some(message) = injected_commit_error {
                transaction
                    .Rollback()
                    .map_err(|error| session_error("rollback injected DML failure", error))?;
                return Err(SessionError::new(message));
            }
            let injected_tso_failures = state.next_autocommit_retry_tso_failures.take();
            state.last_autocommit_retry_attempts =
                self.run_autocommit_commit_retry(injected_tso_failures)?;
            let commit_started = std::time::Instant::now();
            transaction
                .Commit(&kv::Context::default())
                .map_err(|error| session_error("commit autocommit DML", error))?;
            commit_wait = commit_started.elapsed();
            committed = true;
        }
        state.last_dml_report = Some(crate::dml_runtime::DmlExecutionReport {
            Operator: operator.to_owned(),
            Table: SESSION_KV_TABLE.to_owned(),
            AffectedRows: affected_rows,
            LastInsertID: 0,
            WriteKeys: write_keys,
            PrewriteKeys: write_keys,
            Committed: committed,
            CommitWait: commit_wait,
            AllocCount: 0,
            RebaseCount: 0,
            InsertTotalTime: std::time::Duration::ZERO,
            InsertPrepareTime: std::time::Duration::ZERO,
            CheckInsertTime: std::time::Duration::ZERO,
            InsertPrefetchTime: std::time::Duration::ZERO,
            ForeignKeyCheckTime: std::time::Duration::ZERO,
            HasInsertRuntimeStats: false,
            HasForeignKeyChecks: false,
        });
        Ok(())
    }

    /// 执行面向会话 KV 表的 INSERT AST。
    pub(super) fn execute_insert(&self, statement: &ast::InsertStmt) -> SessionResult<()> {
        if statement.Select.is_some() {
            return self.execute_insert_select(statement);
        }
        let plan = crate::dml_runtime::PlanInsert(statement)?;
        if plan.Table != SESSION_KV_TABLE {
            return self.execute_relational_insert(statement, plan, false, None);
        }
        ensure_session_table(&plan.Table)?;
        let mut mutations = Vec::new();
        let mut affected_rows = 0;
        for (key, mut value) in plan.Rows {
            let existing = self.read_session_kv(&key)?;
            if let Some(existing) = existing {
                let existing = String::from_utf8(existing)
                    .map_err(|error| session_error("stored value is not UTF-8", error))?;
                if plan.Replace {
                    affected_rows += 2;
                } else if plan.Ignore {
                    self.set_warning(format!("duplicate session KV key {key}"));
                    continue;
                } else if !plan.OnDuplicate.is_empty() {
                    let mut replacement_key = key.clone();
                    let current_row = HashMap::from([
                        ("k".to_owned(), Some(key.clone())),
                        ("v".to_owned(), Some(existing.clone())),
                    ]);
                    let incoming_row = HashMap::from([
                        ("k".to_owned(), Some(key.clone())),
                        ("v".to_owned(), Some(value.clone())),
                    ]);
                    for (column, expression) in &plan.OnDuplicate {
                        let evaluated = crate::dml_runtime::EvalExpr(
                            expression,
                            &current_row,
                            Some(&incoming_row),
                        )?
                        .ok_or_else(|| SessionError::new("ON DUPLICATE assignment is NULL"))?;
                        if column == "k" {
                            replacement_key = evaluated;
                        } else if column == "v" {
                            value = evaluated;
                        } else {
                            return Err(SessionError::new(format!(
                                "unknown session KV column {column}"
                            )));
                        }
                    }
                    if replacement_key != key {
                        mutations.push((key.clone(), None));
                    }
                    mutations.push((replacement_key, Some(value)));
                    affected_rows += 2;
                    continue;
                } else {
                    return Err(SessionError::new(format!("duplicate session KV key {key}")));
                }
            } else {
                affected_rows += 1;
            }
            mutations.push((key, Some(value)));
        }
        self.apply_session_kv_mutations(
            if plan.Replace { "Replace" } else { "Insert" },
            mutations,
            affected_rows,
        )
    }

    /// 执行面向会话 KV 表的 UPDATE AST。
    pub(super) fn execute_update(&self, statement: &ast::UpdateStmt) -> SessionResult<()> {
        if let Some(sql) = self.mysql_stats_system_update_sql(statement) {
            if self.mysql_stats_histograms_sets_version_zero(statement) {
                self.domain.invalidate_histogram_stats_versions();
            }
            self.domain
                .restricted_stats_execute(&sql, &[])
                .map_err(|error| session_error("update mysql statistics system table", error))?;
            return Ok(());
        }
        let mut sources = Vec::new();
        if let Some(table_refs) = statement.TableRefs.as_ref() {
            if let Some(left) = table_refs.TableRefs.Left.as_deref() {
                collect_physical_table_sources(left, &mut sources);
            }
            if let Some(right) = table_refs.TableRefs.Right.as_deref() {
                collect_physical_table_sources(right, &mut sources);
            }
        }
        if sources.len() > 1 {
            return self.execute_relational_join_update(statement);
        }
        let plan = crate::dml_runtime::PlanUpdate(statement)?;
        if plan.Table != SESSION_KV_TABLE {
            return self.execute_relational_update(plan);
        }
        ensure_session_table(&plan.Table)?;
        if plan.PredicateColumn != "k" || plan.PredicateOp != "=" {
            return Err(SessionError::new(
                "session KV UPDATE requires WHERE k = literal",
            ));
        }
        let Some(value) = self.read_session_kv(&plan.Key)? else {
            return self.apply_session_kv_mutations("Update", Vec::new(), 0);
        };
        let mut key = plan.Key.clone();
        let mut value = String::from_utf8(value)
            .map_err(|error| session_error("stored value is not UTF-8", error))?;
        for (column, expression) in plan.Assignments {
            let evaluated = crate::dml_runtime::EvaluateAssignment(&expression, &key, &value)?;
            if column == "k" {
                key = evaluated;
            } else {
                value = evaluated;
            }
        }
        let mut mutations = Vec::new();
        if key != plan.Key {
            if self.read_session_kv(&key)?.is_some() {
                return Err(SessionError::new(format!("duplicate session KV key {key}")));
            }
            mutations.push((plan.Key, None));
        }
        mutations.push((key, Some(value)));
        self.apply_session_kv_mutations("Update", mutations, 1)
    }

    /// Detect the Go CBO fixture's histogram-version invalidation statement.
    pub(super) fn mysql_stats_histograms_sets_version_zero(
        &self,
        statement: &ast::UpdateStmt,
    ) -> bool {
        let Some(table) = statement.TableRefs.as_ref() else {
            return false;
        };
        let Some(ast::ResultSetNode::TableSource(source)) = table.TableRefs.Left.as_deref() else {
            return false;
        };
        source.Source.Schema.L == "mysql"
            && source.Source.Name.L == "stats_histograms"
            && statement.List.iter().any(|assignment| {
                assignment.Column.Name.L == "stats_ver"
                    && stats_system_literal(&assignment.Expr).ok().as_deref() == Some("0")
            })
    }

    /// 执行面向会话 KV 表的 DELETE AST。
    pub(super) fn execute_delete(&self, statement: &ast::DeleteStmt) -> SessionResult<()> {
        if let Some(sql) = self.mysql_stats_system_delete_sql(statement) {
            // `mysql.column_stats_usage` lives in the statistics handle, so an
            // unrestricted delete must drop the collected usage as well.
            if sql == "delete from mysql.column_stats_usage" {
                self.domain.clear_column_usage();
                return Ok(());
            }
            self.domain
                .restricted_stats_execute(&sql, &[])
                .map_err(|error| session_error("delete mysql statistics system table", error))?;
            return Ok(());
        }
        let mut sources = Vec::new();
        if let Some(table_refs) = statement.TableRefs.as_ref() {
            if let Some(left) = table_refs.TableRefs.Left.as_deref() {
                collect_physical_table_sources(left, &mut sources);
            }
            if let Some(right) = table_refs.TableRefs.Right.as_deref() {
                collect_physical_table_sources(right, &mut sources);
            }
        }
        if sources.len() > 1 {
            return self.execute_relational_join_delete(statement);
        }
        let plan = crate::dml_runtime::PlanDelete(statement)?;
        if plan.Table != SESSION_KV_TABLE {
            return self.execute_relational_delete(plan);
        }
        ensure_session_table(&plan.Table)?;
        if plan.PredicateColumn != "k" || plan.PredicateOp != "=" {
            return Err(SessionError::new(
                "session KV DELETE requires WHERE k = literal",
            ));
        }
        if self.read_session_kv(&plan.Key)?.is_none() {
            return self.apply_session_kv_mutations("Delete", Vec::new(), 0);
        }
        self.apply_session_kv_mutations("Delete", vec![(plan.Key, None)], 1)
    }

    /// 构造 DML EXPLAIN 结果集。
    pub(super) fn explain_dml_record_set(&self) -> SessionResult<ConcreteRecordSet> {
        let report = self
            .state
            .borrow()
            .last_dml_report
            .clone()
            .ok_or_else(|| SessionError::new("EXPLAIN ANALYZE child produced no DML report"))?;
        let allocator = if report.AllocCount == 0 && report.RebaseCount == 0 {
            String::new()
        } else {
            format!(
                ", auto_id_allocator: {{alloc_cnt: {}, rebase_cnt: {}}}",
                report.AllocCount, report.RebaseCount
            )
        };
        let insert_runtime = if report.HasInsertRuntimeStats {
            let foreign_key = if report.HasForeignKeyChecks {
                format!(", fk_check:{:?}", report.ForeignKeyCheckTime)
            } else {
                String::new()
            };
            format!(
                "time:{:?}, loops:1, prepare:{:?}, check_insert: {{total_time:{:?}, mem_insert_time:{:?}, prefetch:{:?}{foreign_key}}}, ",
                report.InsertTotalTime,
                report.InsertPrepareTime,
                report.CheckInsertTime,
                report
                    .CheckInsertTime
                    .saturating_sub(report.InsertPrefetchTime),
                report.InsertPrefetchTime,
            )
        } else {
            String::new()
        };
        let execution_info = format!(
            "{insert_runtime}affected_rows:{}, prewrite_keys:{}, write_keys:{}, commit_wait:{:?}, committed:{}{}",
            report.AffectedRows,
            report.PrewriteKeys,
            report.WriteKeys,
            report.CommitWait,
            report.Committed,
            allocator,
        );
        let mut rows = vec![vec![
            format!("{}_1", report.Operator),
            report.AffectedRows.to_string(),
            report.AffectedRows.to_string(),
            "root".to_owned(),
            format!("table:{}", report.Table),
            execution_info,
            format!("write keys:{}", report.WriteKeys),
            "0 Bytes".to_owned(),
            "0 Bytes".to_owned(),
        ]];
        let current_database = self.current_database();
        let catalog = self.domain.stats_context().catalog();
        let report_table = catalog
            .get(&(current_database.clone(), report.Table.to_ascii_lowercase()))
            .map(|(_, table)| table.clone());
        if let Some(table) = report_table.as_ref() {
            for foreign_key in &table.ForeignKeys {
                let referenced_table = foreign_key.RefTable.O.clone();
                let referenced_index = catalog
                    .get(&(
                        if foreign_key.RefSchema.L.is_empty() {
                            current_database.clone()
                        } else {
                            foreign_key.RefSchema.L.clone()
                        },
                        foreign_key.RefTable.L.clone(),
                    ))
                    .and_then(|(_, parent)| {
                        parent.Indices.iter().find(|index| {
                            index.Columns.len() >= foreign_key.RefCols.len()
                                && index.Columns.iter().zip(&foreign_key.RefCols).all(
                                    |(index_column, fk_column)| index_column.Name.L == fk_column.L,
                                )
                        })
                    })
                    .map(|index| format!(", index:{}", index.Name.O))
                    .unwrap_or_default();
                rows.push(vec![
                    "Foreign_Key_Check_1".to_owned(),
                    "0.00".to_owned(),
                    "0".to_owned(),
                    "root".to_owned(),
                    format!("table:{referenced_table}{referenced_index}"),
                    format!(
                        "total:{:?}, foreign_keys:{}",
                        report.ForeignKeyCheckTime, report.AffectedRows
                    ),
                    format!("foreign_key:{}, check_exist", foreign_key.Name.O),
                    "N/A".to_owned(),
                    "N/A".to_owned(),
                ]);
            }
        }
        if matches!(report.Operator.as_str(), "Delete" | "Update") {
            for ((database, _), (_, child)) in &catalog {
                for foreign_key in child.ForeignKeys.iter().filter(|foreign_key| {
                    foreign_key.RefTable.L == report.Table.to_ascii_lowercase()
                        && (foreign_key.RefSchema.L.is_empty()
                            || foreign_key.RefSchema.L == *database
                            || foreign_key.RefSchema.L == current_database)
                }) {
                    let (cascade_action, action_name) = if report.Operator == "Delete" {
                        match foreign_key.OnDelete {
                            2 => (true, "on_delete:CASCADE"),
                            3 => (true, "on_delete:SET NULL"),
                            _ => (false, "check_not_exist"),
                        }
                    } else if foreign_key.OnUpdate == 2 {
                        (true, "on_update:CASCADE")
                    } else {
                        (false, "check_not_exist")
                    };
                    let child_index = child
                        .Indices
                        .iter()
                        .find(|index| {
                            index.Columns.len() >= foreign_key.Cols.len()
                                && index.Columns.iter().zip(&foreign_key.Cols).all(
                                    |(index_column, fk_column)| index_column.Name.L == fk_column.L,
                                )
                        })
                        .map(|index| format!(", index:{}", index.Name.O))
                        .unwrap_or_default();
                    rows.push(vec![
                        if cascade_action {
                            "Foreign_Key_Cascade_1".to_owned()
                        } else {
                            "Foreign_Key_Check_1".to_owned()
                        },
                        "0.00".to_owned(),
                        "0".to_owned(),
                        "root".to_owned(),
                        format!("table:{}{child_index}", child.Name.O),
                        format!("total:0s, foreign_keys:{}", report.AffectedRows),
                        format!("foreign_key:{}, {action_name}", foreign_key.Name.O),
                        "N/A".to_owned(),
                        "N/A".to_owned(),
                    ]);
                }
            }
        }
        Ok(ConcreteRecordSet::new(
            vec![
                "id".to_owned(),
                "estRows".to_owned(),
                "actRows".to_owned(),
                "task".to_owned(),
                "access object".to_owned(),
                "execution info".to_owned(),
                "operator info".to_owned(),
                "memory".to_owned(),
                "disk".to_owned(),
            ],
            rows,
        ))
    }

    /// Go's `CollectPredicateColumnsPoint`: records predicate-column usage and,
    /// when synchronous loading is disabled, queues the histograms the plan
    /// needs into `AsyncLoadHistogramNeededItems`. Index access paths are
    /// pruned first so pruned indexes never trigger a histogram load.
    /// 从点查谓词收集列信息。
    pub(super) fn collect_predicate_columns_point(
        &self,
        table: &astersql_meta_model::TableInfo,
        statement: &ast::SelectStmt,
    ) -> Result<(), SessionError> {
        // A prepared plan-cache hit skips logical optimization, and with it
        // Go's collection point.
        if self.state.borrow().skip_predicate_collection {
            return Ok(());
        }
        let mut names = BTreeSet::new();
        Self::collect_select_predicate_column_names(statement, &mut names);
        let mut column_ids = table
            .Columns
            .iter()
            .filter(|column| names.contains(&column.Name.L))
            .map(|column| column.ID)
            .collect::<Vec<_>>();
        // A one-column full scan still needs that column's histogram to decide
        // whether the table statistics are usable. This is the issue #48257
        // path: synchronous loading removes `stats:pseudo`, while a zero wait
        // queues the item and keeps pseudo until the async loader publishes it.
        if column_ids.is_empty()
            && statement.Where.is_none()
            && table.Columns.len() == 1
            && statement
                .Fields
                .Fields
                .iter()
                .any(|field| field.WildCard.is_some())
        {
            column_ids.push(table.Columns[0].ID);
        }
        if column_ids.is_empty() {
            return Ok(());
        }
        self.domain
            .record_predicate_column_usage(table.ID, &column_ids);
        // Go's CollectPredicateColumnsPoint queues the histograms a predicate
        // needs for asynchronous loading whenever sync load is disabled
        // (`tidb_stats_load_sync_wait = 0`); otherwise the sync-load path owns
        // the request.
        let referenced = Self::referenced_column_ids(table, statement, &column_ids);
        let kept = self.prune_index_access_paths(table, &column_ids, &referenced);
        // Go `expandStatsNeededColumnsForStaticPruning`: static pruning turns
        // one logical table into one DataSource per partition.
        let static_partition_ids = if self.state.borrow().dynamic_partition_prune {
            Vec::new()
        } else {
            table
                .GetPartitionInfo()
                .map(|partition| {
                    partition
                        .Definitions
                        .iter()
                        .map(|definition| definition.ID)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        for physical_id in std::iter::once(table.ID).chain(static_partition_ids.iter().copied()) {
            let stats = self
                .domain
                .stats_handle()
                .lock()
                .expect("statistics handle")
                .stats_meta(physical_id)
                .cloned();
            let Some(stats) = stats else {
                continue;
            };
            for column_id in &column_ids {
                if stats
                    .columns
                    .get(column_id)
                    .is_some_and(|column| !column.loaded_or_evicted)
                {
                    self.session_vars.StmtCtx.RecordUsedStatsLoadStatus(
                        physical_id,
                        *column_id,
                        false,
                        "allEvicted".to_owned(),
                    );
                }
            }
            for index in &table.Indices {
                let relevant = index.Columns.iter().any(|index_column| {
                    table.Columns.iter().any(|column| {
                        column.Name.L == index_column.Name.L && column_ids.contains(&column.ID)
                    })
                }) && kept.as_ref().is_none_or(|kept| kept.contains(&index.ID));
                if relevant
                    && stats
                        .indexes
                        .get(&index.ID)
                        .is_some_and(|index| !index.fully_loaded)
                {
                    self.session_vars.StmtCtx.RecordUsedStatsLoadStatus(
                        physical_id,
                        index.ID,
                        true,
                        "allEvicted".to_owned(),
                    );
                }
            }
        }
        if self.state.borrow().stats_load_sync_wait != 0 {
            let mut needed = Vec::new();
            for physical_id in std::iter::once(table.ID).chain(static_partition_ids.iter().copied())
            {
                needed.extend(column_ids.iter().map(|column_id| {
                    astersql_sessionctx_stmtctx::cache_value(astersql_meta_model::StatsLoadItem {
                        TableItemID: astersql_meta_model::TableItemID {
                            TableID: physical_id,
                            ID: *column_id,
                            IsIndex: false,
                            IsSyncLoadFailed: false,
                        },
                        FullLoad: true,
                    })
                }));
                needed.extend(
                    table
                        .Indices
                        .iter()
                        .filter(|index| {
                            index.Columns.iter().any(|index_column| {
                                table.Columns.iter().any(|column| {
                                    column.Name.L == index_column.Name.L
                                        && column_ids.contains(&column.ID)
                                })
                            }) && kept.as_ref().is_none_or(|kept| kept.contains(&index.ID))
                        })
                        .map(|index| {
                            astersql_sessionctx_stmtctx::cache_value(
                                astersql_meta_model::StatsLoadItem {
                                    TableItemID: astersql_meta_model::TableItemID {
                                        TableID: physical_id,
                                        ID: index.ID,
                                        IsIndex: true,
                                        IsSyncLoadFailed: false,
                                    },
                                    FullLoad: true,
                                },
                            )
                        }),
                );
            }
            self.session_vars.StmtCtx.ConsumePendingStatsLoadItems();
            self.session_vars
                .StmtCtx
                .StatsLoad
                .NeededItems
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(needed);
            let started = Instant::now();
            let waiter = SessionStatsSyncLoadAdapter::new_with_domain(Arc::clone(&self.domain));
            match astersql_planner_core_base::StatsLoadWaiter::SyncWaitStatsLoad(
                &waiter,
                self.session_vars.as_ref(),
            ) {
                Ok(()) => {
                    self.session_vars
                        .StmtCtx
                        .CompleteStatsSyncWait(started.elapsed());
                    return Ok(());
                }
                Err(error) => {
                    self.session_vars
                        .StmtCtx
                        .FailStatsSyncWait(started.elapsed(), error.clone());
                    if !astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load() {
                        return Err(SessionError::new(error));
                    }
                    self.session_vars.StmtCtx.PlanCacheTracker.SetSkipPlanCache(
                        "synchronous statistics load failed and fell back to pseudo statistics",
                    );
                    self.session_vars
                        .StmtCtx
                        .AppendWarning(astersql_sessionctx_stmtctx::errors::NewNoStackError(error));
                    self.domain.enqueue_async_load_items_after_sync_failure(
                        table,
                        &column_ids,
                        kept.as_ref(),
                        &static_partition_ids,
                    );
                    return Ok(());
                }
            }
        }
        self.domain.enqueue_async_load_items(
            table,
            &column_ids,
            kept.as_ref(),
            &static_partition_ids,
        );
        Ok(())
    }

    /// 在任何 EXPLAIN 特化渲染提前返回前，对语句中的每个物理表执行谓词统计收集。
    pub(super) fn collect_statement_predicate_stats(
        &self,
        statement: &ast::SelectStmt,
    ) -> Result<(), SessionError> {
        if let Some(from) = statement.From.as_ref() {
            let root = ast::ResultSetNode::Join(Box::new(from.TableRefs.clone()));
            let mut sources = Vec::new();
            collect_physical_table_sources(&root, &mut sources);
            let current_database = self.current_database();
            let mut visited = BTreeSet::new();
            for source in sources {
                if let Some(query) = source.QuerySource.as_ref() {
                    query
                        .with_node(|query| self.collect_query_statement_predicate_stats(query))
                        .transpose()?;
                    continue;
                }
                let database = if source.Source.Schema.L.is_empty() {
                    current_database.as_str()
                } else {
                    source.Source.Schema.L.as_str()
                };
                let Some((_, table)) = self.mdl_stats_table(database, &source.Source.Name.L) else {
                    continue;
                };
                if visited.insert(table.ID) {
                    self.collect_predicate_columns_point(&table, statement)?;
                }
            }
        }
        if let Some(with) = statement.With.as_ref().map(|with| with.borrow()) {
            for cte in &with.CTEs {
                self.collect_query_statement_predicate_stats(cte.Query.as_ref())?;
            }
        }
        if let Some(predicate) = statement.Where.as_ref() {
            self.collect_expression_subquery_stats(predicate)?;
        }
        if let Some(having) = statement.Having.as_ref() {
            self.collect_expression_subquery_stats(having)?;
        }
        for field in &statement.Fields.Fields {
            if let Some(expression) = field.Expr.as_ref() {
                self.collect_expression_subquery_stats(expression)?;
            }
        }
        for item in statement.GroupBy.iter().chain(&statement.OrderBy) {
            self.collect_expression_subquery_stats(&item.Expr)?;
        }
        Ok(())
    }

    /// Visit expression subqueries in their own table scope, as the Go
    /// optimizer does when it descends through Apply and semi-join plans.
    fn collect_expression_subquery_stats(
        &self,
        expression: &ast::ExprNode,
    ) -> Result<(), SessionError> {
        struct Collector<'a> {
            session: &'a ConcreteSession,
            error: Option<SessionError>,
        }
        impl ast::ExprNodeVisitor for Collector<'_> {
            fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                if let ast::ExprKind::Subquery { Query, .. } = &input.Kind {
                    if self.error.is_none() {
                        self.error = Query
                            .with_node(|query| {
                                self.session.collect_query_statement_predicate_stats(query)
                            })
                            .transpose()
                            .err();
                    }
                    return (input.clone(), true);
                }
                (input.clone(), false)
            }

            fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                (input.clone(), true)
            }
        }
        let mut collector = Collector {
            session: self,
            error: None,
        };
        let _ = expression.Accept(&mut collector);
        collector.error.map_or(Ok(()), Err)
    }

    /// 递归遍历 CTE/集合运算/派生表查询，触发其中物理表的统计收集。
    fn collect_query_statement_predicate_stats(
        &self,
        query: &dyn ast::Node,
    ) -> Result<(), SessionError> {
        if let Some(select) = query.as_any().downcast_ref::<ast::SelectStmt>() {
            self.collect_statement_predicate_stats(select)
        } else if let Some(statement) = query.as_any().downcast_ref::<ast::SetOprStmt>() {
            if let Some(with) = statement.With.as_ref().map(|with| with.borrow()) {
                for cte in &with.CTEs {
                    self.collect_query_statement_predicate_stats(cte.Query.as_ref())?;
                }
            }
            for select in &statement.select_list.selects {
                self.collect_query_statement_predicate_stats(select.as_ref())?;
            }
            Ok(())
        } else if let Some(set) = query.as_any().downcast_ref::<ast::SetOprSelectList>() {
            if let Some(with) = set.With.as_ref().map(|with| with.borrow()) {
                for cte in &with.CTEs {
                    self.collect_query_statement_predicate_stats(cte.Query.as_ref())?;
                }
            }
            for select in &set.selects {
                self.collect_query_statement_predicate_stats(select.as_ref())?;
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    /// Columns the statement reads, used by Go's `DataSource.IsSingleScan` to
    /// decide whether an index path avoids the table lookup.
    /// 收集表达式引用的列 ID。
    pub(super) fn referenced_column_ids(
        table: &astersql_meta_model::TableInfo,
        statement: &ast::SelectStmt,
        predicate_column_ids: &[i64],
    ) -> BTreeSet<i64> {
        let mut referenced = predicate_column_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        for field in &statement.Fields.Fields {
            if field.WildCard.is_some() {
                return table.Columns.iter().map(|column| column.ID).collect();
            }
            let Some(expr) = field.Expr.as_ref() else {
                continue;
            };
            let mut names = BTreeSet::new();
            Self::collect_predicate_column_names(expr, &mut names);
            referenced.extend(
                table
                    .Columns
                    .iter()
                    .filter(|column| names.contains(&column.Name.L))
                    .map(|column| column.ID),
            );
        }
        referenced
    }

    /// Go `pruneIndexesForDataSource`. Returns the surviving index IDs, or
    /// `None` when nothing was pruned so `collectSyncIndices` keeps its
    /// unrestricted behavior.
    /// 按谓词裁剪索引访问路径。
    pub(super) fn prune_index_access_paths(
        &self,
        table: &astersql_meta_model::TableInfo,
        predicate_column_ids: &[i64],
        referenced_column_ids: &BTreeSet<i64>,
    ) -> Option<BTreeSet<i64>> {
        let threshold = self.state.borrow().opt_index_prune_threshold;
        if threshold < 0 {
            return None;
        }
        let usable = table
            .Indices
            .iter()
            .filter(|index| index.State == astersql_meta_model::StatePublic && !index.Invisible)
            .collect::<Vec<_>>();
        // A DataSource always owns the table access path plus one path per
        // usable index; a single path disables pruning altogether.
        if usable.is_empty() {
            return None;
        }
        let table_column_ids = table
            .Columns
            .iter()
            .map(|column| column.ID)
            .collect::<Vec<_>>();
        let offset_of = |name: &ast::CIStr| {
            table
                .Columns
                .iter()
                .position(|column| column.Name.L == name.L)
        };
        let mut paths = Vec::with_capacity(usable.len() + 1);
        paths.push(prune_indexes::AccessPath {
            id: 0,
            index: None,
            table_path: true,
            forced: false,
            full_index_columns: None,
            single_scan: true,
        });
        for (position, index) in usable.iter().enumerate() {
            let offsets = index
                .Columns
                .iter()
                .map(|column| offset_of(&column.Name))
                .collect::<Vec<_>>();
            let index_column_ids = offsets
                .iter()
                .map(|offset| offset.map(|offset| table_column_ids[offset]))
                .collect::<Vec<_>>();
            let single_scan = !referenced_column_ids.is_empty()
                && referenced_column_ids
                    .iter()
                    .all(|id| index_column_ids.contains(&Some(*id)));
            paths.push(prune_indexes::AccessPath {
                id: position as u64 + 1,
                index: Some(prune_indexes::IndexInfo {
                    id: index.ID,
                    name: index.Name.L.clone(),
                    columns: offsets
                        .iter()
                        .flatten()
                        .map(|offset| prune_indexes::IndexColumn { offset: *offset })
                        .collect(),
                    multi_value: index.MVIndex,
                    condition_expression: (!index.ConditionExprString.is_empty())
                        .then(|| index.ConditionExprString.clone()),
                    affected_column_offsets: index
                        .AffectColumn
                        .iter()
                        .flatten()
                        .filter_map(|column| offset_of(&column.Name))
                        .collect(),
                }),
                table_path: false,
                forced: false,
                full_index_columns: Some(index_column_ids),
                single_scan,
            });
        }
        let total_paths = paths.len();
        // Go treats threshold 0 as "only prune the zero-score indexes".
        let threshold = if threshold == 0 {
            total_paths as isize
        } else {
            threshold as isize
        };
        let source = prune_indexes::DataSource {
            table_columns: table_column_ids,
            ..prune_indexes::DataSource::default()
        };
        let kept = prune_indexes::prune_indexes_by_where_and_order(
            &source,
            paths,
            predicate_column_ids,
            threshold,
        );
        if kept.len() >= total_paths {
            return None;
        }
        Some(
            kept.iter()
                .filter_map(|path| path.index.as_ref().map(|index| index.id))
                .collect(),
        )
    }

    /// Reads a `@name` user variable or a `@@name` system variable projected by
    /// a constant SELECT.
    /// 读取用户变量或系统变量值。
    pub(super) fn select_variable(&self, name: &str, is_system: bool) -> SessionResult<String> {
        let name = name.trim_start_matches('@').to_lowercase();
        if !is_system {
            return Ok(self
                .state
                .borrow()
                .user_variables
                .get(&name)
                .cloned()
                .unwrap_or_else(|| SHOW_NULL_CELL.to_owned()));
        }
        let global_scope = name.starts_with("global.");
        let name = name
            .strip_prefix("session.")
            .or_else(|| name.strip_prefix("local."))
            .or_else(|| name.strip_prefix("global."))
            .unwrap_or(&name)
            .to_owned();
        if name == astersql_sessionctx_vardef::TiDBPagingSizeBytes {
            return Ok(astersql_sessionctx_vardef::PagingSizeBytes
                .Load()
                .to_string());
        }
        if name == astersql_sessionctx_vardef::TiDBMergePartitionStatsConcurrency {
            return Ok("1".into());
        }
        if astersql_sessionctx_variable::is_embedding_api_key(&name) {
            return Ok(astersql_sessionctx_variable::mask_embedding_api_key(
                &self
                    .domain
                    .global_system_variable(&name)
                    .unwrap_or_default(),
            ));
        }
        if name == astersql_sessionctx_variable::EMBEDDING_API_BASE {
            let value = self
                .domain
                .global_system_variable(&name)
                .unwrap_or_default();
            return Ok(if value.is_empty() {
                astersql_sessionctx_variable::DEFAULT_EMBEDDING_API_BASE.to_owned()
            } else {
                value
            });
        }
        // Match Go SysVar.GetNativeValType for these numeric booleans:
        // SQL exposes integers while mysql.global_variables retains ON/OFF.
        if matches!(
            name.as_str(),
            astersql_sessionctx_vardef::TiDBOptAdvancedJoinHint
                | astersql_sessionctx_vardef::TiDBEnableINLJoinInnerMultiPattern
                | astersql_sessionctx_vardef::TiDBEnableRateLimitAction
        ) || (global_scope && name == "autocommit")
        {
            let value = if global_scope {
                self.domain
                    .global_system_variable(&name)
                    .or_else(|| self.session_vars.GetSystemVar(&name))
            } else {
                self.session_vars.GetSystemVar(&name)
            }
            .ok_or_else(|| SessionError::new(format!("Unknown system variable '{name}'")))?;
            return Ok(u8::from(astersql_sessionctx_variable::TiDBOptOn(&value)).to_string());
        }
        if name == astersql_sessionctx_vardef::TiDBStatsLoadPseudoTimeout {
            // GLOBAL getter reads the live atomic; SQL's native Bool is 0/1.
            return Ok(
                u8::from(astersql_sessionctx_vardef::StatsLoadPseudoTimeout.Load()).to_string(),
            );
        }
        if global_scope && name == astersql_sessionctx_vardef::TiDBGCLifetime {
            let lifetime = super::session::load_external_gc_lifetime(&self.domain)?;
            return Ok(astersql_sessionctx_variable::format_go_duration(
                lifetime.as_nanos() as i128,
            ));
        }
        if global_scope
            && !matches!(name.as_str(), "tx_read_only" | "transaction_read_only")
            && let Some(value) = self.domain.global_system_variable(&name)
        {
            // Go's registered sysvar getter returns the normalized textual
            // value (for example `ON`/`OFF`) instead of coercing every Bool
            // variable to a numeric SQL boolean. Variables whose public
            // contract is numeric have explicit branches below.
            return Ok(value.clone());
        }
        match name.as_str() {
            astersql_sessionctx_vardef::TiDBEnableMDL => {
                Ok(u8::from(astersql_sessionctx_vardef::IsMDLEnabled()).to_string())
            }
            "last_insert_id" if !global_scope => {
                Ok(self.state.borrow().info_last_insert_id.to_string())
            }
            "autocommit" if !global_scope => {
                Ok(u8::from(self.state.borrow().autocommit).to_string())
            }
            astersql_sessionctx_vardef::WarningCount => {
                Ok(self.state.borrow().last_warnings.len().to_string())
            }
            astersql_sessionctx_vardef::ErrorCount => Ok(self
                .state
                .borrow()
                .last_warnings
                .iter()
                .filter(|warning| warning.level == "Error")
                .count()
                .to_string()),
            astersql_sessionctx_vardef::TiDBLastQueryInfo => {
                Ok(self.state.borrow().last_query_info.clone())
            }
            "version_comment" => Ok("AsterSQL Server".to_owned()),
            // Go `LastPlanFromCache`, a read-only status variable.
            "last_plan_from_cache" => {
                Ok(u8::from(self.state.borrow().last_plan_from_cache).to_string())
            }
            astersql_sessionctx_vardef::TiDBUseAlloc => {
                Ok(u8::from(self.session_vars.StmtCtx.GetUseChunkAllocStatus()).to_string())
            }
            astersql_sessionctx_vardef::TiDBEnableInstancePlanCache => Ok(u8::from(
                astersql_sessionctx_vardef::EnableInstancePlanCache.Load(),
            )
            .to_string()),
            astersql_sessionctx_vardef::TiDBInstancePlanCacheReservedPercentage => Ok(
                astersql_sessionctx_vardef::InstancePlanCacheReservedPercentage
                    .Load()
                    .to_string(),
            ),
            astersql_sessionctx_vardef::TiDBInstancePlanCacheMaxMemSize => {
                Ok(astersql_sessionctx_vardef::InstancePlanCacheMaxMemSize
                    .Load()
                    .to_string())
            }
            "tidb_enable_prepared_plan_cache" => {
                Ok(u8::from(self.state.borrow().prepared_plan_cache).to_string())
            }
            "tidb_enable_non_prepared_plan_cache" => {
                Ok(u8::from(self.state.borrow().non_prepared_plan_cache).to_string())
            }
            astersql_sessionctx_vardef::TiDBAllowMPPExecution => {
                Ok(u8::from(self.state.borrow().allow_mpp).to_string())
            }
            astersql_sessionctx_vardef::TiDBEnforceMPPExecution => {
                Ok(u8::from(self.state.borrow().enforce_mpp).to_string())
            }
            astersql_sessionctx_vardef::TiDBOptTiFlashConcurrencyFactor => {
                let value = self.state.borrow().tiflash_concurrency_factor;
                Ok(if value.fract() == 0.0 {
                    (value as i64).to_string()
                } else {
                    value.to_string()
                })
            }
            astersql_sessionctx_vardef::TiFlashComputeDispatchPolicy => {
                let policy = if global_scope {
                    self.domain.global_tiflash_compute_dispatch_policy()
                } else {
                    self.state.borrow().tiflash_compute_dispatch_policy
                };
                Ok(
                    astersql_sessionctx_variable::tiflashcompute::GetDispatchPolicy(policy)
                        .to_owned(),
                )
            }
            "sql_mode" => Ok(self.state.borrow().sql_mode.clone()),
            "auto_increment_increment" => {
                Ok(self.state.borrow().auto_increment_increment.to_string())
            }
            "auto_increment_offset" => Ok(self.state.borrow().auto_increment_offset.to_string()),
            "timestamp" => {
                if let Some(value) = self.state.borrow().timestamp_override {
                    Ok(if value.fract() == 0.0 {
                        (value as i64).to_string()
                    } else {
                        value.to_string()
                    })
                } else {
                    Ok(SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64()
                        .to_string())
                }
            }
            "max_connections" => Ok(self.state.borrow().max_connections.to_string()),
            "max_prepared_stmt_count" => {
                Ok(runtime_max_prepared_stmt_count(&self.domain).to_string())
            }
            "tx_read_only" | "transaction_read_only" => Ok(u8::from(if global_scope {
                self.state.borrow().global_tx_read_only
            } else {
                self.state.borrow().tx_read_only
            })
            .to_string()),
            "tidb_scatter_region" => {
                if global_scope {
                    Ok(self.domain.global_scatter_region())
                } else {
                    Ok(self.state.borrow().scatter_region.clone())
                }
            }
            "tidb_txn_mode" => {
                if global_scope {
                    Ok(self.domain.global_txn_mode())
                } else {
                    Ok(self.state.borrow().txn_mode.clone())
                }
            }
            "tidb_current_ts" => Ok(self.current_store_ts()?.to_string()),
            "tidb_last_txn_info" => Ok(format!(
                "{{\"commit_ts\":{}}}",
                self.state.borrow().last_commit_ts
            )),
            "tx_isolation" | "transaction_isolation" => {
                Ok(self.state.borrow().transaction_isolation.clone())
            }
            "tx_isolation_one_shot" => Ok(self
                .state
                .borrow()
                .transaction_isolation_one_shot
                .clone()
                .unwrap_or_default()),
            astersql_sessionctx_vardef::CharacterSetResults => {
                Ok(self.state.borrow().character_set_results.clone())
            }
            "tidb_enable_async_commit" => {
                Ok(u8::from(self.state.borrow().enable_async_commit).to_string())
            }
            "tidb_guarantee_linearizability" => {
                Ok(u8::from(self.state.borrow().guarantee_linearizability).to_string())
            }
            "tidb_enable_1pc" => Ok(u8::from(self.state.borrow().enable_1pc).to_string()),
            "tidb_replica_read" => Ok(self.state.borrow().replica_read.clone()),
            "tidb_enable_paging" => Ok(u8::from(self.state.borrow().enable_paging).to_string()),
            astersql_sessionctx_vardef::TiDBMemArbitratorMode => {
                Ok(GetGlobalMemArbitratorWorkModeText())
            }
            astersql_sessionctx_vardef::TiDBMemArbitratorSoftLimit => {
                Ok(GetGlobalMemArbitratorSoftLimitText())
            }
            astersql_sessionctx_vardef::TiDBMemArbitratorWaitAverse => {
                Ok(self.state.borrow().mem_arbitrator_wait_averse.clone())
            }
            astersql_sessionctx_vardef::TiDBMemArbitratorQueryReserved => Ok(self
                .state
                .borrow()
                .statement_mem_arbitrator_query_reserved
                .unwrap_or(self.state.borrow().mem_arbitrator_query_reserved)
                .to_string()),
            "tidb_server_memory_limit" => Ok(astersql_util_memory::tracker::ServerMemoryLimit
                .Load()
                .to_string()),
            astersql_sessionctx_vardef::TiDBDDLErrorCountLimit => {
                Ok(astersql_sessionctx_vardef::GetDDLErrorCountLimit().to_string())
            }
            astersql_sessionctx_vardef::TiDBDDLReorgMaxWriteSpeed => {
                Ok(astersql_sessionctx_vardef::DDLReorgMaxWriteSpeed
                    .Load()
                    .to_string())
            }
            astersql_sessionctx_vardef::TiDBEnableDistTask => {
                Ok(u8::from(astersql_sessionctx_vardef::EnableDistTask.Load()).to_string())
            }
            _ => {
                let value = self.session_vars.GetSystemVar(&name).ok_or_else(|| {
                    SessionError::new(format!("Unknown system variable '{name}'"))
                })?;
                Ok(value.clone())
            }
        }
    }

    /// 收集谓词中出现的列名。
    pub(super) fn collect_predicate_column_names(
        expression: &ast::ExprNode,
        names: &mut BTreeSet<String>,
    ) {
        struct Collector<'a> {
            names: &'a mut BTreeSet<String>,
        }
        impl ast::ExprNodeVisitor for Collector<'_> {
            fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                match &input.Kind {
                    ast::ExprKind::Column(column) => {
                        self.names.insert(column.Name.L.clone());
                    }
                    // A subquery owns a separate logical-plan scope. Its
                    // predicate columns are collected when that query is
                    // planned/executed, and must not be attributed to the
                    // outer table merely because column names coincide.
                    ast::ExprKind::Subquery { .. } => return (input.clone(), true),
                    _ => {}
                }
                (input.clone(), false)
            }

            fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                (input.clone(), true)
            }
        }
        let _ = expression.Accept(&mut Collector { names });
    }

    /// 收集 SELECT、JOIN、子查询与 CTE 中所有会影响计划选择的列名。
    fn collect_select_predicate_column_names(
        statement: &ast::SelectStmt,
        names: &mut BTreeSet<String>,
    ) {
        fn collect_join(join: &ast::Join, names: &mut BTreeSet<String>) {
            if let Some(on) = join.On.as_ref() {
                ConcreteSession::collect_predicate_column_names(on, names);
            }
            names.extend(join.Using.iter().map(|column| column.Name.L.clone()));
            for source in [join.Left.as_deref(), join.Right.as_deref()]
                .into_iter()
                .flatten()
            {
                match source {
                    ast::ResultSetNode::Join(nested) => collect_join(nested, names),
                    ast::ResultSetNode::TableSource(source) => {
                        if let Some(query) = source.QuerySource.as_ref() {
                            query.with_node(|query| {
                                ConcreteSession::collect_query_predicate_column_names(query, names);
                            });
                        }
                    }
                }
            }
        }

        if let Some(predicate) = statement.Where.as_ref() {
            Self::collect_predicate_column_names(predicate, names);
        }
        if let Some(having) = statement.Having.as_ref() {
            Self::collect_predicate_column_names(having, names);
        }
        for item in statement.GroupBy.iter().chain(&statement.OrderBy) {
            Self::collect_predicate_column_names(&item.Expr, names);
        }
        if let Some(from) = statement.From.as_ref() {
            collect_join(&from.TableRefs, names);
        }
        if let Some(with) = statement.With.as_ref().map(|with| with.borrow()) {
            for cte in &with.CTEs {
                Self::collect_query_predicate_column_names(cte.Query.as_ref(), names);
            }
        }
    }

    /// 收集 SELECT 或集合运算查询节点中的计划相关列。
    fn collect_query_predicate_column_names(query: &dyn ast::Node, names: &mut BTreeSet<String>) {
        if let Some(select) = query.as_any().downcast_ref::<ast::SelectStmt>() {
            Self::collect_select_predicate_column_names(select, names);
        } else if let Some(statement) = query.as_any().downcast_ref::<ast::SetOprStmt>() {
            for select in &statement.select_list.selects {
                Self::collect_query_predicate_column_names(select.as_ref(), names);
            }
            if let Some(with) = statement.With.as_ref().map(|with| with.borrow()) {
                for cte in &with.CTEs {
                    Self::collect_query_predicate_column_names(cte.Query.as_ref(), names);
                }
            }
        } else if let Some(set) = query.as_any().downcast_ref::<ast::SetOprSelectList>() {
            for select in &set.selects {
                Self::collect_query_predicate_column_names(select.as_ref(), names);
            }
            if let Some(with) = set.With.as_ref().map(|with| with.borrow()) {
                for cte in &with.CTEs {
                    Self::collect_query_predicate_column_names(cte.Query.as_ref(), names);
                }
            }
        }
    }

    /// 解析 ANALYZE 目标列信息。
    pub(super) fn analyze_columns_info(
        &self,
        statement: &ast::AnalyzeTableStmt,
        key: &astersql_statistics_handle::StatsTableKey,
        info: &astersql_meta_model::TableInfo,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        // Go `getMustAnalyzedColumns`: indexed columns plus the integer handle.
        let must_analyze = info
            .Indices
            .iter()
            .filter(|index| {
                (index.State == astersql_meta_model::SchemaState::Public
                    || (index.State == astersql_meta_model::SchemaState::WriteReorganization
                        && self.session_vars.EnableDDLAnalyzeExecOpt))
                    && !index.MVIndex
            })
            .flat_map(|index| index.Columns.iter())
            .filter_map(|column| {
                info.Columns
                    .iter()
                    .find(|candidate| candidate.Name.L == column.Name.L)
                    .map(|candidate| candidate.ID)
            })
            .chain(
                info.PKIsHandle
                    .then(|| info.GetPkColInfo().map(|column| column.ID))
                    .flatten(),
            )
            .collect::<BTreeSet<_>>();
        let choice = match statement.ColumnChoice {
            ast::ColumnChoice::Default => astersql_sessionctx_vardef::AnalyzeColumnOptions.Load(),
            ast::ColumnChoice::All => "ALL".to_owned(),
            ast::ColumnChoice::Predicate => "PREDICATE".to_owned(),
            ast::ColumnChoice::List => "LIST".to_owned(),
        };
        let requested = statement
            .ColumnNames
            .iter()
            .filter_map(|name| {
                info.Columns
                    .iter()
                    .find(|column| column.Name.L == name.L)
                    .map(|column| column.ID)
            })
            .collect::<BTreeSet<_>>();
        let invalid = statement
            .ColumnNames
            .iter()
            .filter(|name| !info.Columns.iter().any(|column| column.Name.L == name.L))
            .map(|name| name.O.clone())
            .collect::<Vec<_>>();
        let selected = match choice.to_ascii_uppercase().as_str() {
            "PREDICATE" => {
                let predicate = self
                    .domain
                    .stats_context()
                    .column_usage()
                    .into_iter()
                    .filter(|usage| usage.table_id == key.table_id && usage.last_used_at.is_some())
                    .map(|usage| usage.column_id)
                    .collect::<BTreeSet<_>>();
                predicate
                    .union(&must_analyze)
                    .copied()
                    .collect::<BTreeSet<_>>()
            }
            "LIST" => requested
                .clone()
                .union(&must_analyze)
                .copied()
                .collect::<BTreeSet<_>>(),
            _ => info.Columns.iter().map(|column| column.ID).collect(),
        };
        let missing = if choice.eq_ignore_ascii_case("LIST") {
            info.Columns
                .iter()
                .filter(|column| {
                    must_analyze.contains(&column.ID) && !requested.contains(&column.ID)
                })
                .map(|column| column.Name.O.clone())
                .collect()
        } else {
            Vec::new()
        };
        let skip_types = self.domain.stats_session_vars().analyze_skip_column_types;
        let analyzed = info
            .Columns
            .iter()
            .filter(|column| selected.contains(&column.ID))
            .filter(|column| {
                // Go hardcodes the vector type as never collectable.
                if column.FieldType.GetType()
                    == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                {
                    return false;
                }
                let type_name = astersql_parser_types::TypeToStr(
                    column.FieldType.GetType(),
                    &column.FieldType.GetCharset(),
                );
                !skip_types.contains(&type_name) || must_analyze.contains(&column.ID)
            })
            .map(|column| column.Name.O.clone())
            .collect();
        (analyzed, missing, invalid)
    }

    pub(super) fn evaluate_set_expression(
        &self,
        expression: &ast::ExprNode,
    ) -> SessionResult<String> {
        match &expression.Kind {
            ast::ExprKind::Variable {
                Name,
                IsGlobal,
                IsSystem,
                ..
            } => {
                let scoped_name = if *IsGlobal {
                    format!("global.{Name}")
                } else {
                    Name.clone()
                };
                self.select_variable(&scoped_name, *IsSystem)
            }
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("tidb_parse_tso") && Args.len() == 1 =>
            {
                self.evaluate_set_expression(&Args[0])
            }
            ast::ExprKind::Function { FnName, .. } if FnName.L.eq_ignore_ascii_case("now") => {
                Ok(format_system_time(SystemTime::now()))
            }
            ast::ExprKind::Function { .. } => {
                relational_expression_value(expression, &HashMap::new())?
                    .ok_or_else(|| SessionError::new("SET function expression evaluated to NULL"))
            }
            ast::ExprKind::Column(column) => Ok(column.Name.O.clone()),
            ast::ExprKind::Binary { Op, L, R } if matches!(Op.as_str(), "+" | "-" | "*" | "/") => {
                let left = self
                    .evaluate_set_expression(L)?
                    .parse::<i128>()
                    .map_err(|error| session_error("parse SET arithmetic left operand", error))?;
                let right = self
                    .evaluate_set_expression(R)?
                    .parse::<i128>()
                    .map_err(|error| session_error("parse SET arithmetic right operand", error))?;
                let value = match Op.as_str() {
                    "+" => left.checked_add(right),
                    "-" => left.checked_sub(right),
                    "*" => left.checked_mul(right),
                    "/" if right != 0 => left.checked_div(right),
                    "/" => return Err(SessionError::new("division by zero in SET expression")),
                    _ => unreachable!("operator checked above"),
                }
                .ok_or_else(|| SessionError::new("integer overflow in SET expression"))?;
                Ok(value.to_string())
            }
            _ => literal(expression),
        }
    }

    pub(super) fn current_store_ts(&self) -> SessionResult<u64> {
        if let Some(timestamp) = self.state.borrow().transaction_stale_read_ts {
            return Ok(timestamp);
        }
        let Some(timestamp) = self
            .state
            .borrow()
            .transaction
            .as_ref()
            .map(|transaction| transaction.StartTS())
        else {
            // Go returns SessionVars.TxnCtx.StartTS, whose value is zero when
            // there is no active transaction. Reading this variable must not
            // allocate a new TSO or implicitly start a transaction.
            let mut state = self.state.borrow_mut();
            let databases = state.databases.clone();
            state
                .tso_catalog_versions
                .insert(0, self.domain.stats_context().catalog_version());
            state.tso_database_names.insert(0, databases);
            return Ok(0);
        };
        let mut state = self.state.borrow_mut();
        let databases = state.databases.clone();
        state
            .tso_catalog_versions
            .insert(timestamp, self.domain.stats_context().catalog_version());
        state.tso_database_names.insert(timestamp, databases);
        state.last_observed_store_ts = timestamp;
        state.tso_wall_times.push((SystemTime::now(), timestamp));
        Ok(timestamp)
    }

    /// Reject a normal table read whose start timestamp precedes this session's
    /// last successful commit. Explicit stale/snapshot reads intentionally use
    /// older timestamps and therefore bypass this linearizability check.
    pub(super) fn validate_table_read_ts_after_last_commit(&self) -> SessionResult<()> {
        let (last_commit_ts, transaction_start_ts, stale_read) = {
            let state = self.state.borrow();
            (
                state.last_commit_ts,
                state
                    .transaction
                    .as_ref()
                    .map(|transaction| transaction.StartTS()),
                state.transaction_stale_read_ts.is_some()
                    || state.pending_stale_read_ts.is_some()
                    || state.session_stale_read_ts.is_some()
                    || state.snapshot_read_ts.is_some(),
            )
        };
        if last_commit_ts == 0 || stale_read {
            return Ok(());
        }
        let start_ts = if let Some(start_ts) = transaction_start_ts {
            start_ts
        } else {
            self.domain
                .storage()
                .with_storage(|store| store.CurrentVersion("global"))
                .map_err(|error| session_error("read current version", error))?
                .Ver
        };
        if start_ts < last_commit_ts {
            return Err(SessionError::new(format!(
                "start_ts:{start_ts} is before session last_commit_ts:{last_commit_ts}"
            )));
        }
        Ok(())
    }

    pub(super) fn evaluate_stale_read_ts(&self, expression: &ast::ExprNode) -> SessionResult<u64> {
        fn wall_clock_tso() -> u64 {
            (SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64)
                << 18
        }

        fn observe_stale_tso(timestamp: u64) -> SessionResult<u64> {
            if let Some(expected) = astersql_testkit_testfailpoint::eval_string(
                "github.com/pingcap/tidb/pkg/executor/assertStaleTSO",
            )
            .and_then(|value| value.parse::<u64>().ok())
            {
                let actual = (timestamp >> 18) / 1_000;
                if actual != expected {
                    return Err(SessionError::new(format!(
                        "stale TSO physical time mismatch: expected {expected}, got {actual}"
                    )));
                }
            }
            Ok(timestamp)
        }

        fn dynamic_ts(expression: &ast::ExprNode, now_ts: u64) -> Option<u64> {
            match &expression.Kind {
                ast::ExprKind::Parentheses(inner) => dynamic_ts(inner, now_ts),
                ast::ExprKind::Value(value) => {
                    let text = value.text();
                    let text = text.trim_matches(['\'', '"']);
                    text.parse::<u64>().ok().or_else(|| {
                        parse_stale_datetime_micros(text)
                            .ok()
                            .map(|micros| ((micros.max(0) as u64) / 1_000) << 18)
                    })
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if matches!(
                        FnName.L.as_str(),
                        "now" | "current_timestamp" | "current_time" | "curtime"
                    ) =>
                {
                    Some(now_ts)
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if matches!(FnName.L.as_str(), "date_sub" | "subdate") && Args.len() >= 3 =>
                {
                    let base = dynamic_ts(&Args[0], now_ts)?;
                    let amount = literal(&Args[1]).ok()?.parse::<u64>().ok()?;
                    let ast::ExprKind::TimeUnit(unit) = Args[2].Kind else {
                        return None;
                    };
                    let unit_millis = unit.duration_nanos()? / 1_000_000;
                    Some(
                        base.saturating_sub(
                            amount.saturating_mul(unit_millis).saturating_mul(1 << 18),
                        ),
                    )
                }
                _ => None,
            }
        }

        let stale_now_tso = || {
            if let Some(timestamp) = self.state.borrow().current_stale_now_ts {
                return timestamp;
            }
            let timestamp = astersql_testkit_testfailpoint::eval_string(
                "github.com/pingcap/tidb/pkg/sessiontxn/staleread/mockStaleReadTSO",
            )
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| {
                astersql_testkit_testfailpoint::eval_string(
                    "github.com/pingcap/tidb/pkg/expression/injectNow",
                )
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1_000) << 18)
            })
            .unwrap_or_else(wall_clock_tso);
            self.state.borrow_mut().current_stale_now_ts = Some(timestamp);
            timestamp
        };

        if let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind
            && FnName.L.eq_ignore_ascii_case("timestamp")
            && Args.len() == 1
        {
            return self.evaluate_stale_read_ts(&Args[0]);
        }
        if let ast::ExprKind::Variable {
            Name,
            IsSystem: false,
            ..
        } = &expression.Kind
        {
            let name = Name.trim_start_matches('@').to_lowercase();
            let value = self
                .state
                .borrow()
                .user_variables
                .get(&name)
                .cloned()
                .ok_or_else(|| SessionError::new(format!("user variable @{name} is not set")))?;
            let value = value.trim_matches(['\'', '"']);
            if let Ok(timestamp) = value.parse::<u64>() {
                return observe_stale_tso(timestamp);
            }
            let micros = parse_stale_datetime_micros(value)
                .map_err(|error| session_error("parse stale read timestamp", error))?;
            return observe_stale_tso(((micros.max(0) as u64) / 1_000) << 18);
        }

        let text = match &expression.Kind {
            ast::ExprKind::Value(value) => value.text(),
            ast::ExprKind::Parentheses(inner) => return self.evaluate_stale_read_ts(inner),
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L == "tidb_bounded_staleness" && Args.len() == 2 =>
            {
                let now_ts = stale_now_tso();
                let lower = dynamic_ts(&Args[0], now_ts)
                    .ok_or_else(|| SessionError::new("invalid bounded-staleness lower bound"))?;
                let upper = dynamic_ts(&Args[1], now_ts)
                    .ok_or_else(|| SessionError::new("invalid bounded-staleness upper bound"))?;
                let safe_ts = astersql_testkit_testfailpoint::eval_string(
                    "github.com/pingcap/tidb/pkg/expression/injectSafeTS",
                )
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_else(|| {
                    let labels = astersql_config::get_global_config().labels.clone();
                    let txn_scope = runtime_txn_scope(&labels);
                    self.domain
                        .storage()
                        .with_storage(|store| store.GetMinSafeTS(&txn_scope))
                });
                return observe_stale_tso(safe_ts.clamp(lower, upper));
            }
            _ => {
                let now_ts = stale_now_tso();
                let timestamp = dynamic_ts(expression, now_ts).ok_or_else(|| {
                    SessionError::new("unsupported stale-read timestamp expression")
                })?;
                return observe_stale_tso(timestamp);
            }
        };
        let text = text.trim_matches(['\'', '"']);
        if let Ok(timestamp) = text.parse::<u64>() {
            if (timestamp >> 18) > (wall_clock_tso() >> 18) {
                return Err(SessionError::new(
                    "cannot set read timestamp to a future time",
                ));
            }
            return observe_stale_tso(timestamp);
        }
        let micros = parse_stale_datetime_micros(text)
            .map_err(|error| session_error("parse stale read timestamp", error))?;
        let now_micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i128;
        if i128::from(micros) > now_micros {
            return Err(SessionError::new(
                "cannot set read timestamp to a future time",
            ));
        }
        observe_stale_tso(((micros.max(0) as u64) / 1_000) << 18)
    }

    pub(super) fn execute_mem_arbitrator_set(
        &self,
        is_global: bool,
        name: &str,
        value: &str,
    ) -> SessionResult<bool> {
        const ERR_MODE: &str = "tidb_mem_arbitrator_mode: disable; standard; priority;";
        const ERR_SOFT_LIMIT: &str = "tidb_mem_arbitrator_soft_limit: 0 (default); (0, 1.0] float-rate * server-limit; (1, server-limit] integer bytes; auto;";
        const ERR_WAIT_AVERSE: &str =
            "tidb_mem_arbitrator_wait_averse: 0 (disable); 1 (enable); nolimit;";
        const ERR_QUERY_RESERVED: &str =
            "tidb_mem_arbitrator_query_reserved: 0 (default); (1, server-limit] integer bytes;";

        let value = value.trim_matches(['\'', '"']).to_ascii_lowercase();
        match name {
            astersql_sessionctx_vardef::TiDBMemArbitratorMode => {
                if !is_global {
                    return Err(SessionError::new(
                        "tidb_mem_arbitrator_mode is a GLOBAL variable",
                    ));
                }
                if !matches!(value.as_str(), "disable" | "standard" | "priority") {
                    return Err(SessionError::new(ERR_MODE));
                }
                let _ = SetGlobalMemArbitratorWorkMode(value);
                Ok(true)
            }
            astersql_sessionctx_vardef::TiDBMemArbitratorSoftLimit => {
                if !is_global {
                    return Err(SessionError::new(
                        "tidb_mem_arbitrator_soft_limit is a GLOBAL variable",
                    ));
                }
                let valid = matches!(value.as_str(), "0" | "auto")
                    || value.parse::<u64>().is_ok_and(|bytes| bytes > 1)
                    || value
                        .parse::<f64>()
                        .is_ok_and(|ratio| ratio > 0.0 && ratio <= 1.0);
                if !valid {
                    return Err(SessionError::new(ERR_SOFT_LIMIT));
                }
                SetGlobalMemArbitratorSoftLimit(value);
                Ok(true)
            }
            astersql_sessionctx_vardef::TiDBMemArbitratorWaitAverse => {
                if is_global {
                    return Err(SessionError::new(
                        "tidb_mem_arbitrator_wait_averse is a SESSION variable",
                    ));
                }
                if !matches!(value.as_str(), "0" | "1" | "nolimit") {
                    return Err(SessionError::new(ERR_WAIT_AVERSE));
                }
                self.state.borrow_mut().mem_arbitrator_wait_averse = value;
                Ok(true)
            }
            astersql_sessionctx_vardef::TiDBMemArbitratorQueryReserved => {
                if is_global {
                    return Err(SessionError::new(
                        "tidb_mem_arbitrator_query_reserved is a SESSION variable",
                    ));
                }
                let reserved = value
                    .parse::<i64>()
                    .ok()
                    .filter(|reserved| *reserved == 0 || *reserved > 1)
                    .filter(|reserved| {
                        let limit = astersql_util_memory::tracker::ServerMemoryLimit.Load() as i64;
                        limit == 0 || *reserved <= limit
                    })
                    .ok_or_else(|| SessionError::new(ERR_QUERY_RESERVED))?;
                self.state.borrow_mut().mem_arbitrator_query_reserved = reserved;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn execute_create_resource_group(
        &self,
        statement: &ast::CreateResourceGroupStmt,
    ) -> SessionResult<()> {
        let mut group = RuntimeResourceGroup {
            ru_per_sec: 0,
            burst_limit: 0,
            priority: ArbitrationPriorityMedium,
        };
        for option in &statement.ResourceGroupOptionList {
            match option.Tp {
                ast::ResourceGroupOptionType::RURate => {
                    group.ru_per_sec = if option.Burstable == ast::BurstableType::Unlimited {
                        astersql_meta_model::group_3::unlimitedRURate
                    } else {
                        option.UintValue
                    };
                }
                ast::ResourceGroupOptionType::Burstable => {
                    group.burst_limit = match option.Burstable {
                        ast::BurstableType::Disable => 0,
                        ast::BurstableType::Moderated => -2,
                        ast::BurstableType::Unlimited => -1,
                    };
                }
                ast::ResourceGroupOptionType::Priority => {
                    group.priority = match option.UintValue {
                        16 => ArbitrationPriorityHigh,
                        1 => ArbitrationPriorityLow,
                        _ => ArbitrationPriorityMedium,
                    };
                }
                _ => {}
            }
        }
        let name = statement.ResourceGroupName.L.clone();
        let domain_id = runtime_domain_id(&self.domain);
        let mut groups = RUNTIME_RESOURCE_GROUPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let groups = groups.entry(domain_id).or_default();
        if groups.contains_key(&name) {
            if statement.IfNotExists {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::note_with_code(
                        astersql_errno::errcode::ErrResourceGroupExists,
                        format!("Resource group '{name}' already exists"),
                    ));
                return Ok(());
            }
            return Err(SessionError::new(format!(
                "resource group {name} already exists"
            )));
        }
        groups.insert(name, group);
        Ok(())
    }

    pub(super) fn execute_alter_resource_group(
        &self,
        statement: &ast::AlterResourceGroupStmt,
    ) -> SessionResult<()> {
        let domain_id = runtime_domain_id(&self.domain);
        let name = statement.ResourceGroupName.L.clone();
        if name == astersql_resourcegroup::DEFAULT_RESOURCE_GROUP_NAME {
            if let Some(background) = statement
                .ResourceGroupOptionList
                .iter()
                .find(|option| option.Tp == ast::ResourceGroupOptionType::Background)
                .and_then(|option| default_resource_group_background(&option.BackgroundOptions))
            {
                persist_default_resource_group_background(&self.domain, background)?;
            }
        }
        let mut groups = RUNTIME_RESOURCE_GROUPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let groups = groups.entry(domain_id).or_default();
        if name == astersql_resourcegroup::DEFAULT_RESOURCE_GROUP_NAME {
            groups.entry(name.clone()).or_insert(RuntimeResourceGroup {
                ru_per_sec: i32::MAX as u64,
                burst_limit: -1,
                priority: ArbitrationPriorityMedium,
            });
        }
        let Some(group) = groups.get_mut(&name) else {
            if statement.IfExists {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::note_with_code(
                        astersql_errno::errcode::ErrResourceGroupNotExists,
                        format!("Unknown resource group '{name}'"),
                    ));
                return Ok(());
            }
            return Err(SessionError::new(format!(
                "resource group {name} does not exist"
            )));
        };
        for option in &statement.ResourceGroupOptionList {
            match option.Tp {
                ast::ResourceGroupOptionType::RURate => {
                    group.ru_per_sec = if option.Burstable == ast::BurstableType::Unlimited {
                        astersql_meta_model::group_3::unlimitedRURate
                    } else {
                        option.UintValue
                    };
                }
                ast::ResourceGroupOptionType::Burstable => {
                    group.burst_limit = match option.Burstable {
                        ast::BurstableType::Disable => 0,
                        ast::BurstableType::Moderated => -2,
                        ast::BurstableType::Unlimited => -1,
                    };
                }
                ast::ResourceGroupOptionType::Priority => {
                    group.priority = match option.UintValue {
                        16 => ArbitrationPriorityHigh,
                        1 => ArbitrationPriorityLow,
                        _ => ArbitrationPriorityMedium,
                    };
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn execute_drop_resource_group(
        &self,
        statement: &ast::DropResourceGroupStmt,
    ) -> SessionResult<()> {
        let domain_id = runtime_domain_id(&self.domain);
        let mut groups = RUNTIME_RESOURCE_GROUPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let groups = groups.entry(domain_id).or_default();
        let name = statement.ResourceGroupName.L.clone();
        if groups.remove(&name).is_none() && !statement.IfExists {
            return Err(SessionError::new(format!(
                "resource group {name} does not exist"
            )));
        }
        Ok(())
    }

    /// Persist TLS requirements inside the account mutation's transaction.
    fn persist_account_tls(
        &self,
        privileges: &mut astersql_privilege_privileges::MySQLPrivilege,
        user: &str,
        host: &str,
        value: &str,
    ) -> SessionResult<()> {
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"));
        self.execute_privilege_mutation(&format!(
            "INSERT INTO mysql.global_priv (Host, User, Priv) VALUES ({}, {}, {}) ON DUPLICATE KEY UPDATE Priv=values(Priv)",
            quote(host), quote(user), quote(value),
        ))?;
        let value = serde_json::from_str(value)
            .map_err(|error| session_error("decode account TLS requirements", error))?;
        let row = HashMap::from([
            ("host".to_owned(), serde_json::json!(host)),
            ("user".to_owned(), serde_json::json!(user)),
            ("priv".to_owned(), value),
        ]);
        privileges
            .global_priv
            .retain(|record| !record.base.fullyMatch(user, host));
        privileges
            .decodeGlobalPrivTableRow(&row)
            .map_err(|error| session_error("cache account TLS requirements", error))?;
        privileges
            .global_priv
            .sort_by(astersql_privilege_privileges::compareGlobalPrivRecord);
        Ok(())
    }

    /// ALTER options supported by the concrete account persistence path.
    pub(super) fn execute_alter_user(&self, statement: &ast::AlterUserStmt) -> SessionResult<()> {
        if statement.CurrentAuth.is_some()
            || statement.CurrentDualPasswordOption != ast::DualPasswordOptionType::None
            || statement.Specs.iter().any(|spec| {
                spec.AuthOpt.is_some()
                    || spec.User.current_user
                    || spec.DualPasswordOption != ast::DualPasswordOptionType::None
            })
            || !statement.ResourceOptions.is_empty()
            || statement.ResourceGroupNameOption.is_some()
            || statement.PasswordOrLockOptions.iter().any(|option| {
                !matches!(
                    option.Type,
                    ast::PasswordOrLockOptionType::Lock
                        | ast::PasswordOrLockOptionType::Unlock
                        | ast::PasswordOrLockOptionType::PasswordExpire
                )
            })
        {
            return Err(SessionError::new(
                "statement requires the full planner/executor session ABI",
            ));
        }
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        let caller = self.login_user.as_deref().unwrap_or("root");
        let caller_host = self.authenticated_host.as_deref().unwrap_or("%");
        let skip_grant_table = astersql_config::get_global_config()
            .security
            .skip_grant_table;
        if !skip_grant_table
            && !privileges.RequestVerification(
                &self.active_roles.borrow(),
                caller,
                caller_host,
                "",
                "",
                "",
                astersql_privilege_privileges::CreateUserPriv,
            )
            && !privileges.RequestVerification(
                &self.active_roles.borrow(),
                caller,
                caller_host,
                "mysql",
                "user",
                "",
                astersql_privilege_privileges::UpdatePriv,
            )
        {
            return Err(SessionError::new(
                "[planner:1227]Access denied; you need (at least one of) the CREATE USER privilege(s) for this operation",
            ));
        }
        let can_alter_system_user = skip_grant_table
            || privileges.RequestDynamicVerification(
                &self.active_roles.borrow(),
                caller,
                caller_host,
                "SYSTEM_USER",
                false,
            );
        let can_alter_restricted_user = skip_grant_table
            || privileges.RequestDynamicVerification(
                &self.active_roles.borrow(),
                caller,
                caller_host,
                "RESTRICTED_USER_ADMIN",
                false,
            );
        let priv_data = astersql_executor::grant::account_tls_options_to_global_priv(
            &statement.AuthTokenOrTLSOptions,
        )
        .map_err(|error| session_error("ALTER USER REQUIRE", error))?;
        let lock = statement
            .PasswordOrLockOptions
            .iter()
            .rev()
            .find_map(|option| match option.Type {
                ast::PasswordOrLockOptionType::Lock => Some(true),
                ast::PasswordOrLockOptionType::Unlock => Some(false),
                _ => None,
            });
        let expire = statement
            .PasswordOrLockOptions
            .iter()
            .any(|option| option.Type == ast::PasswordOrLockOptionType::PasswordExpire);
        let metadata = statement
            .CommentOrAttributeOption
            .as_ref()
            .map(|option| {
                let metadata = match option.Type {
                    ast::CommentOrAttributeOptionType::UserComment => {
                        serde_json::json!({"comment": option.Value})
                    }
                    ast::CommentOrAttributeOptionType::UserAttribute => {
                        serde_json::from_str(&option.Value)
                            .map_err(|error| session_error("ALTER USER ATTRIBUTE", error))?
                    }
                };
                Ok::<_, SessionError>(metadata)
            })
            .transpose()?;
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"));
        let mut failed_users = Vec::new();
        let mut need_rollback = false;
        let mut create_sql_updates = Vec::new();
        self.begin_transaction(&ast::BeginStmt::default())?;
        let result = (|| {
            for spec in &statement.Specs {
                let user = &spec.User.username;
                let host = account_host(&spec.User.hostname).to_ascii_lowercase();
                let identity = format!("'{}'@'{}'", user, host);
                let Some(index) = privileges
                    .user
                    .iter()
                    .position(|record| record.base.fullyMatch(user, &host))
                else {
                    failed_users.push(identity);
                    continue;
                };
                let roles = privileges.getAllRoles(user, &host);
                if !can_alter_system_user
                    && !can_alter_restricted_user
                    && privileges.RequestDynamicVerification(
                        &roles,
                        user,
                        &host,
                        "SYSTEM_USER",
                        false,
                    )
                {
                    return Err(SessionError::new(
                        "[planner:1227]Access denied; you need (at least one of) the SYSTEM_USER or SUPER privilege(s) for this operation",
                    ));
                }
                if astersql_util_sem_compat::IsEnabled()
                    && !can_alter_restricted_user
                    && privileges.RequestDynamicVerification(
                        &roles,
                        user,
                        &host,
                        "RESTRICTED_USER_ADMIN",
                        false,
                    )
                {
                    return Err(SessionError::new(
                        "[planner:1227]Access denied; you need (at least one of) the RESTRICTED_USER_ADMIN privilege(s) for this operation",
                    ));
                }
                let mut record = privileges.user[index].clone();
                let mut fields = Vec::new();
                if let Some(locked) = lock {
                    fields.push(format!(
                        "account_locked='{}'",
                        if locked { "Y" } else { "N" }
                    ));
                    record.AccountLocked = locked;
                }
                if expire {
                    fields.push("password_expired='Y'".to_owned());
                    record.PasswordExpired = true;
                }
                if let Some(metadata) = metadata.as_ref() {
                    fields.push(format!(
                        "user_attributes=json_merge_patch(coalesce(user_attributes, '{{}}'), {})",
                        quote(&serde_json::json!({"metadata": metadata}).to_string()),
                    ));
                }
                if let Some(issuer) = statement
                    .AuthTokenOrTLSOptions
                    .iter()
                    .find(|option| option.Type == ast::AuthTokenOrTLSOptionType::TokenIssuer)
                {
                    if record.AuthPlugin == "tidb_auth_token" {
                        fields.push(format!("token_issuer={}", quote(&issuer.Value)));
                        record.AuthTokenIssuer = issuer.Value.clone();
                    } else {
                        self.state
                            .borrow_mut()
                            .current_warnings
                            .push(SessionWarning::warning(
                                "TOKEN_ISSUER is not needed for the auth plugin".to_owned(),
                            ));
                    }
                }
                if !fields.is_empty() {
                    if self
                        .execute_privilege_mutation(&format!(
                            "UPDATE mysql.user SET {} WHERE Host={} AND User={}",
                            fields.join(", "),
                            quote(&host),
                            quote(user),
                        ))
                        .is_err()
                    {
                        failed_users.push(identity);
                        need_rollback = true;
                        continue;
                    }
                }
                // Omission preserves existing requirements. A token-only REQUIRE has no TLS value.
                if !statement.AuthTokenOrTLSOptions.is_empty()
                    && let Some(value) = priv_data.as_deref().filter(|value| !value.is_empty())
                    && self
                        .persist_account_tls(&mut privileges, user, &host, value)
                        .is_err()
                {
                    failed_users.push(identity);
                    need_rollback = true;
                    continue;
                }
                privileges.user[index] = record.clone();
                let require = privileges
                    .global_priv
                    .iter()
                    .find(|record| record.base.fullyMatch(user, &host))
                    .map(|record| record.Priv.RequireStr())
                    .unwrap_or_else(|| "NONE".to_owned());
                let token = if record.AuthTokenIssuer.is_empty() {
                    String::new()
                } else {
                    format!(" token_issuer {}", record.AuthTokenIssuer)
                };
                let authentication = if record.AuthenticationString.is_empty()
                    && record.AuthPlugin == "auth_socket"
                {
                    format!("IDENTIFIED WITH '{}'", record.AuthPlugin)
                } else {
                    format!(
                        "IDENTIFIED WITH '{}' AS '{}'",
                        record.AuthPlugin.replace('\'', "''"),
                        record.AuthenticationString.replace('\'', "''")
                    )
                };
                let mut create = format!(
                    "CREATE USER '{}'@'{}' {authentication} REQUIRE {require}{token} PASSWORD EXPIRE {}ACCOUNT {} PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT",
                    user.replace('\'', "''"),
                    host.replace('\'', "''"),
                    if record.PasswordExpired {
                        ""
                    } else {
                        "DEFAULT "
                    },
                    if record.AccountLocked {
                        "LOCK"
                    } else {
                        "UNLOCK"
                    },
                );
                if let Some(metadata) = metadata.as_ref() {
                    create.push_str(" ATTRIBUTE '");
                    create.push_str(
                        &metadata
                            .to_string()
                            .replace(":", ": ")
                            .replace(",", ", ")
                            .replace('\'', "''"),
                    );
                    create.push('\'');
                } else if let Some(existing) = RUNTIME_CREATE_USER_SQL
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&runtime_domain_id(&self.domain))
                    .and_then(|accounts| accounts.get(&(user.clone(), host.clone())))
                    .and_then(|sql| {
                        sql.split_once(" ATTRIBUTE ")
                            .map(|(_, attribute)| attribute.to_owned())
                    })
                {
                    create.push_str(" ATTRIBUTE ");
                    create.push_str(&existing);
                }
                create_sql_updates.push(((user.clone(), host), create));
            }
            if !failed_users.is_empty() && (!statement.IfExists || need_rollback) {
                return Err(SessionError::new(format!(
                    "[executor:1396]Operation ALTER USER failed for {}",
                    failed_users.join(","),
                )));
            }
            for user in &failed_users {
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning::note_with_code(
                        astersql_errno::errcode::ErrBadUser,
                        format!("User {user} does not exist."),
                    ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.finish_transaction(false)?;
            return Err(error);
        }
        self.finish_transaction(true)?;
        privileges.SortUserTable();
        handle.merge(privileges);
        RUNTIME_CREATE_USER_SQL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(runtime_domain_id(&self.domain))
            .or_default()
            .extend(create_sql_updates);
        Ok(())
    }

    pub(super) fn execute_create_user(&self, statement: &ast::CreateUserStmt) -> SessionResult<()> {
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        let default_plugin = self
            .select_variable(astersql_sessionctx_vardef::DefaultAuthPlugin, true)
            .unwrap_or_else(|_| astersql_parser_mysql::r#const::AuthNativePassword.to_owned());
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"));
        let priv_data = astersql_executor::grant::account_tls_options_to_global_priv(
            &statement.AuthTokenOrTLSOptions,
        )
        .map_err(|error| session_error("CREATE USER REQUIRE", error))?;
        let token_issuer = statement
            .AuthTokenOrTLSOptions
            .iter()
            .find(|option| option.Type == ast::AuthTokenOrTLSOptionType::TokenIssuer)
            .map(|option| option.Value.as_str())
            .unwrap_or_default();
        let mut persisted_rows = Vec::new();
        let mut new_users = Vec::new();
        for spec in &statement.Specs {
            let user = &spec.User.username;
            let host = account_host(&spec.User.hostname).to_ascii_lowercase();
            if privileges
                .user
                .iter()
                .any(|record| record.base.fullyMatch(user, &host))
            {
                if statement.IfNotExists {
                    continue;
                }
                return Err(SessionError::new(format!(
                    "user '{user}'@'{host}' already exists"
                )));
            }
            let (password, valid) =
                astersql_executor::utils::encodePasswordWithPlugin(spec, None, &default_plugin);
            if !valid {
                return Err(SessionError::new(format!(
                    "invalid password hash for user '{user}'@'{host}'"
                )));
            }
            let plugin = spec
                .AuthOpt
                .as_ref()
                .map(|option| option.AuthPlugin.as_str())
                .filter(|plugin| !plugin.is_empty())
                .unwrap_or(&default_plugin);
            let record_token_issuer = if plugin == "tidb_auth_token" {
                if token_issuer.is_empty() {
                    self.state.borrow_mut().current_warnings.push(SessionWarning::warning(
                        "TOKEN_ISSUER is needed for 'tidb_auth_token' user, please use 'alter user' to declare it".to_owned(),
                    ));
                }
                token_issuer
            } else {
                if !token_issuer.is_empty() {
                    self.state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::warning(format!(
                            "TOKEN_ISSUER is not needed for '{plugin}' user"
                        )));
                }
                ""
            };
            persisted_rows.push(format!(
                "({}, {}, {}, {}, {})",
                quote(&host),
                quote(user),
                quote(&password),
                quote(plugin),
                quote(record_token_issuer),
            ));
            new_users.push((
                host,
                user.clone(),
                password,
                plugin.to_owned(),
                record_token_issuer.to_owned(),
            ));
        }
        if persisted_rows.is_empty() {
            return Ok(());
        }

        let insert_sql = format!(
            "INSERT {}INTO mysql.user (Host, User, authentication_string, plugin, Token_issuer) VALUES {}",
            if statement.IfNotExists { "IGNORE " } else { "" },
            persisted_rows.join(", "),
        );
        let statements = parse(&insert_sql)?;
        let insert = statements
            .first()
            .and_then(|statement| statement.as_any().downcast_ref::<ast::InsertStmt>())
            .ok_or_else(|| SessionError::new("CREATE USER persistence did not parse as INSERT"))?;
        self.begin_transaction(&ast::BeginStmt::default())?;
        let result = (|| {
            self.execute_insert(insert)?;
            if let Some(value) = priv_data.as_deref().filter(|value| !value.is_empty()) {
                for (host, user, _, _, _) in &new_users {
                    self.persist_account_tls(&mut privileges, user, host, value)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.finish_transaction(false)?;
            return Err(error);
        }
        self.finish_transaction(true)?;

        let account_locked = statement
            .PasswordOrLockOptions
            .iter()
            .rev()
            .find_map(|option| match option.Type {
                ast::PasswordOrLockOptionType::Lock => Some(true),
                ast::PasswordOrLockOptionType::Unlock => Some(false),
                _ => None,
            })
            .unwrap_or(false);
        let attribute =
            statement
                .CommentOrAttributeOption
                .as_ref()
                .map(|option| match option.Type {
                    ast::CommentOrAttributeOptionType::UserComment => serde_json::json!({
                        "comment": option.Value,
                    })
                    .to_string()
                    .replace(":", ": "),
                    ast::CommentOrAttributeOptionType::UserAttribute => {
                        serde_json::from_str::<serde_json::Value>(&option.Value)
                            .unwrap_or_else(|_| serde_json::json!({}))
                            .to_string()
                            .replace(":", ": ")
                            .replace(",", ", ")
                    }
                });
        for (host, user, password, plugin, token_issuer) in new_users {
            if !privileges
                .user
                .iter()
                .any(|record| record.base.fullyMatch(&user, &host))
            {
                let mut record = astersql_privilege_privileges::NewUserRecord(&host, &user);
                record.AuthenticationString = password.clone();
                record.AuthPlugin = plugin.clone();
                record.AccountLocked = account_locked;
                record.AuthTokenIssuer = token_issuer.clone();
                privileges.user.push(record);
            }
            let authentication = if password.is_empty() && plugin == "auth_socket" {
                format!("IDENTIFIED WITH '{plugin}'")
            } else {
                format!(
                    "IDENTIFIED WITH '{}' AS '{}'",
                    plugin.replace('\'', "''"),
                    password.replace('\'', "''")
                )
            };
            let require = privileges
                .global_priv
                .iter()
                .find(|record| record.base.fullyMatch(&user, &host))
                .map(|record| record.Priv.RequireStr())
                .unwrap_or_else(|| "NONE".to_owned());
            let token = if token_issuer.is_empty() {
                String::new()
            } else {
                format!(" token_issuer {token_issuer}")
            };
            let mut create = format!(
                "CREATE USER '{}'@'{}' {authentication} REQUIRE {require}{token} PASSWORD EXPIRE DEFAULT ACCOUNT {} PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT",
                user.replace('\'', "''"),
                host.replace('\'', "''"),
                if account_locked { "LOCK" } else { "UNLOCK" }
            );
            if let Some(attribute) = attribute.as_deref() {
                create.push_str(" ATTRIBUTE '");
                create.push_str(&attribute.replace('\'', "''"));
                create.push('\'');
            }
            RUNTIME_CREATE_USER_SQL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(runtime_domain_id(&self.domain))
                .or_default()
                .insert((user.clone(), host.clone()), create);
        }
        privileges.SortUserTable();
        handle.merge(privileges);
        Ok(())
    }

    pub(super) fn execute_grant_role(&self, statement: &ast::GrantRoleStmt) -> SessionResult<()> {
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        for user in &statement.Users {
            let user = astersql_privilege_privileges::RoleIdentity::new(
                &user.username,
                account_host(&user.hostname),
            );
            let edges = privileges.role_graph.entry(user).or_default();
            for role in &statement.Roles {
                edges
                    .0
                    .insert(astersql_privilege_privileges::RoleIdentity::new(
                        &role.username,
                        account_host(&role.hostname),
                    ));
            }
        }
        handle.merge(privileges);
        Ok(())
    }

    pub(super) fn execute_set_role(&self, statement: &ast::SetRoleStmt) -> SessionResult<()> {
        let Some(user) = self.login_user.as_deref() else {
            return Err(SessionError::new("SET ROLE requires an authenticated user"));
        };
        let host = self.authenticated_host.as_deref().unwrap_or("%");
        let privileges = runtime_privilege_handle(&self.domain).Get();
        let requested = match statement.SetRoleOpt {
            ast::SetRoleOpt::None => Vec::new(),
            ast::SetRoleOpt::All => privileges.getAllRoles(user, host),
            ast::SetRoleOpt::Regular | ast::SetRoleOpt::Default => statement
                .RoleList
                .iter()
                .map(|role| {
                    astersql_privilege_privileges::RoleIdentity::new(
                        &role.username,
                        account_host(&role.hostname),
                    )
                })
                .collect(),
            ast::SetRoleOpt::AllExcept => {
                let excluded = statement
                    .RoleList
                    .iter()
                    .map(|role| {
                        astersql_privilege_privileges::RoleIdentity::new(
                            &role.username,
                            account_host(&role.hostname),
                        )
                    })
                    .collect::<HashSet<_>>();
                privileges
                    .getAllRoles(user, host)
                    .into_iter()
                    .filter(|role| !excluded.contains(role))
                    .collect()
            }
        };
        if let Some(role) = requested
            .iter()
            .find(|role| !privileges.FindRole(user, host, role))
        {
            return Err(SessionError::new(format!(
                "role {}@{} is not granted to {}@{}",
                role.Username, role.Hostname, user, host
            )));
        }
        *self.active_roles.borrow_mut() = requested;
        Ok(())
    }

    pub(super) fn execute_set_default_role(
        &self,
        statement: &ast::SetDefaultRoleStmt,
    ) -> SessionResult<()> {
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        for user in &statement.UserList {
            let host = account_host(&user.hostname);
            let granted = privileges.getAllRoles(&user.username, host);
            let requested = match statement.SetRoleOpt {
                ast::SetRoleOpt::None => Vec::new(),
                ast::SetRoleOpt::All => granted.clone(),
                ast::SetRoleOpt::Regular | ast::SetRoleOpt::Default => statement
                    .RoleList
                    .iter()
                    .map(|role| {
                        astersql_privilege_privileges::RoleIdentity::new(
                            &role.username,
                            account_host(&role.hostname),
                        )
                    })
                    .collect(),
                ast::SetRoleOpt::AllExcept => {
                    let excluded = statement
                        .RoleList
                        .iter()
                        .map(|role| {
                            astersql_privilege_privileges::RoleIdentity::new(
                                &role.username,
                                account_host(&role.hostname),
                            )
                        })
                        .collect::<HashSet<_>>();
                    granted
                        .into_iter()
                        .filter(|role| !excluded.contains(role))
                        .collect()
                }
            };
            if let Some(role) = requested
                .iter()
                .find(|role| !privileges.FindRole(&user.username, host, role))
            {
                return Err(SessionError::new(format!(
                    "role {}@{} is not granted to {}@{}",
                    role.Username, role.Hostname, user.username, host
                )));
            }
            privileges.setDefaultRoles(&user.username, host, &requested);
        }
        handle.merge(privileges);
        Ok(())
    }

    pub(super) fn execute_grant(&self, statement: &ast::GrantStmt) -> SessionResult<()> {
        use astersql_parser_mysql::privs as mysql;
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        let database = if statement.Level.DBName.is_empty() {
            self.current_database()
        } else {
            statement.Level.DBName.clone()
        };
        if statement.Level.Level != ast::GrantLevelType::Global && database.is_empty() {
            return Err(SessionError::new("No database selected for GRANT"));
        }
        let table = statement.Level.TableName.clone();
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"));
        let user_columns = mysql::Priv2UserCol();
        let mut static_mask = mysql::PrivilegeType(0);
        let mut column_grants = Vec::new();
        let allowed = match statement.Level.Level {
            ast::GrantLevelType::Global => mysql::AllGlobalPrivs(),
            ast::GrantLevelType::DB => mysql::AllDBPrivs(),
            ast::GrantLevelType::Table => mysql::AllTablePrivs(),
        };
        for privilege in &statement.Privs {
            if !privilege.Name.is_empty() {
                if statement.Level.Level != ast::GrantLevelType::Global {
                    return Err(SessionError::new("dynamic privileges require global scope"));
                }
                continue;
            }
            let granted = if privilege.Priv == mysql::AllPriv {
                allowed.clone()
            } else if privilege.Priv == mysql::UsagePriv {
                Vec::new()
            } else if privilege.Cols.is_empty() && allowed.contains(&privilege.Priv) {
                vec![privilege.Priv]
            } else if statement.Level.Level == ast::GrantLevelType::Table
                && mysql::AllColumnPrivs().contains(&privilege.Priv)
                && !privilege.Cols.is_empty()
            {
                for column in &privilege.Cols {
                    column_grants.push((column.Name.O.clone(), privilege.Priv));
                }
                Vec::new()
            } else {
                return Err(SessionError::new("illegal privilege level for GRANT"));
            };
            for privilege in granted {
                static_mask = mysql::PrivilegeType(static_mask.0 | privilege.0);
            }
        }
        if statement.WithGrant {
            static_mask = mysql::PrivilegeType(static_mask.0 | mysql::GrantPriv.0);
        }

        for spec in &statement.Users {
            let host = account_host(&spec.User.hostname).to_ascii_lowercase();
            if !privileges
                .user
                .iter()
                .any(|record| record.base.fullyMatch(&spec.User.username, &host))
            {
                return Err(SessionError::new(format!(
                    "Unknown user: '{}'@'{}'",
                    spec.User.username, host
                )));
            }
        }

        self.ensure_implicit_transaction()?;
        let result = (|| {
            for spec in &statement.Users {
                let user = &spec.User.username;
                let host = account_host(&spec.User.hostname).to_ascii_lowercase();
                match statement.Level.Level {
                    ast::GrantLevelType::Global => {
                        let assignments = mysql::AllGlobalPrivs()
                            .into_iter()
                            .chain(std::iter::once(mysql::GrantPriv))
                            .filter(|privilege| static_mask.0 & privilege.0 != 0)
                            .filter_map(|privilege| user_columns.get(&privilege))
                            .map(|column| format!("{column}='Y'"))
                            .collect::<Vec<_>>();
                        if !assignments.is_empty() {
                            let columns = assignments
                                .iter()
                                .map(|assignment| assignment.split('=').next().unwrap_or_default())
                                .collect::<Vec<_>>();
                            self.execute_privilege_mutation(&format!(
                                "INSERT INTO mysql.user (Host, User, {}) VALUES ({}, {}, {}) ON DUPLICATE KEY UPDATE {}",
                                columns.join(", "), quote(&host), quote(user),
                                vec!["'Y'"; columns.len()].join(", "), assignments.join(", ")
                            ))?;
                        }
                        for privilege in &statement.Privs {
                            if !privilege.Name.is_empty() {
                                self.execute_privilege_mutation(&format!(
                                    "REPLACE INTO mysql.global_grants (User, Host, Priv, With_grant_option) VALUES ({}, {}, {}, {})",
                                    quote(user), quote(&host), quote(&privilege.Name.to_ascii_uppercase()),
                                    quote(if statement.WithGrant { "Y" } else { "N" })
                                ))?;
                            }
                        }
                    }
                    ast::GrantLevelType::DB => {
                        let columns = mysql::AllDBPrivs()
                            .into_iter()
                            .chain(std::iter::once(mysql::GrantPriv))
                            .filter(|privilege| static_mask.0 & privilege.0 != 0)
                            .filter_map(|privilege| user_columns.get(&privilege).copied())
                            .collect::<Vec<_>>();
                        let assignments = columns
                            .iter()
                            .map(|column| format!("{column}='Y'"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let sql = if columns.is_empty() {
                            format!(
                                "INSERT IGNORE INTO mysql.db (Host, DB, User) VALUES ({}, {}, {})",
                                quote(&host),
                                quote(&database),
                                quote(user)
                            )
                        } else {
                            format!(
                                "INSERT INTO mysql.db (Host, DB, User, {}) VALUES ({}, {}, {}, {}) ON DUPLICATE KEY UPDATE {assignments}",
                                columns.join(", "),
                                quote(&host),
                                quote(&database),
                                quote(user),
                                vec!["'Y'"; columns.len()].join(", ")
                            )
                        };
                        self.execute_privilege_mutation(&sql)?;
                    }
                    ast::GrantLevelType::Table => {
                        if static_mask != mysql::PrivilegeType(0) {
                            let old = privileges.tables_priv.iter().find(|record| {
                                record.base.fullyMatch(user, &host)
                                    && record.DB.eq_ignore_ascii_case(&database)
                                    && record.TableName.eq_ignore_ascii_case(&table)
                            });
                            let mut old_table_names =
                                astersql_privilege_privileges::EncodePrivilegeSet(
                                    old.map_or(0, |record| record.TablePriv),
                                    astersql_privilege_privileges::ALL_TABLE_PRIVS,
                                );
                            if old.is_some_and(|record| {
                                record.TablePriv & astersql_privilege_privileges::GrantPriv != 0
                            }) {
                                old_table_names.push("Grant".to_owned());
                            }
                            let old_column_names =
                                astersql_privilege_privileges::EncodePrivilegeSet(
                                    old.map_or(0, |record| record.ColumnPriv),
                                    astersql_privilege_privileges::ALL_TABLE_PRIVS,
                                );
                            let table_set = mysql::AllTablePrivs()
                                .into_iter()
                                .chain(std::iter::once(mysql::GrantPriv))
                                .filter(|privilege| {
                                    static_mask.0 & privilege.0 != 0
                                        || old_table_names.iter().any(|name| {
                                            name.eq_ignore_ascii_case(privilege.SetString())
                                        })
                                })
                                .map(mysql::PrivilegeType::SetString)
                                .collect::<Vec<_>>()
                                .join(",");
                            let column_set = mysql::AllColumnPrivs()
                                .into_iter()
                                .filter(|privilege| {
                                    static_mask.0 & privilege.0 != 0
                                        || old_column_names.iter().any(|name| {
                                            name.eq_ignore_ascii_case(privilege.SetString())
                                        })
                                })
                                .map(mysql::PrivilegeType::SetString)
                                .collect::<Vec<_>>()
                                .join(",");
                            self.execute_privilege_mutation(&format!(
                                "INSERT INTO mysql.tables_priv (Host, DB, User, Table_name, Table_priv, Column_priv) VALUES ({}, {}, {}, {}, {}, {}) ON DUPLICATE KEY UPDATE Table_priv={}, Column_priv={}",
                                quote(&host), quote(&database), quote(user), quote(&table), quote(&table_set), quote(&column_set), quote(&table_set), quote(&column_set)
                            ))?;
                        }
                        for (column, mask) in &column_grants {
                            let old_mask = privileges
                                .columns_priv
                                .iter()
                                .find(|record| {
                                    record.base.fullyMatch(user, &host)
                                        && record.DB.eq_ignore_ascii_case(&database)
                                        && record.TableName.eq_ignore_ascii_case(&table)
                                        && record.ColumnName.eq_ignore_ascii_case(column)
                                })
                                .map_or(0, |record| record.ColumnPriv);
                            let old_names = astersql_privilege_privileges::EncodePrivilegeSet(
                                old_mask,
                                astersql_privilege_privileges::ALL_TABLE_PRIVS,
                            );
                            let value = mysql::AllColumnPrivs()
                                .into_iter()
                                .filter(|privilege| {
                                    privilege.0 & mask.0 != 0
                                        || old_names.iter().any(|name| {
                                            name.eq_ignore_ascii_case(privilege.SetString())
                                        })
                                })
                                .map(mysql::PrivilegeType::SetString)
                                .collect::<Vec<_>>()
                                .join(",");
                            self.execute_privilege_mutation(&format!(
                                "INSERT INTO mysql.columns_priv (Host, DB, User, Table_name, Column_name, Column_priv) VALUES ({}, {}, {}, {}, {}, {}) ON DUPLICATE KEY UPDATE Column_priv={}",
                                quote(&host), quote(&database), quote(user), quote(&table), quote(column), quote(&value), quote(&value)
                            ))?;
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.finish_transaction(false)?;
            return Err(error);
        }
        self.finish_transaction(true)?;

        for spec in &statement.Users {
            let user = &spec.User.username;
            let host = account_host(&spec.User.hostname).to_ascii_lowercase();
            match statement.Level.Level {
                ast::GrantLevelType::Global => {
                    let columns = mysql::AllGlobalPrivs()
                        .into_iter()
                        .chain(std::iter::once(mysql::GrantPriv))
                        .filter(|privilege| static_mask.0 & privilege.0 != 0)
                        .map(|privilege| privilege.ColumnString().to_owned())
                        .collect::<Vec<_>>();
                    privileges.GrantGlobalPrivilegeMask(
                        &host,
                        user,
                        astersql_privilege_privileges::DecodePrivilegeColumns(&columns),
                    );
                }
                ast::GrantLevelType::DB => {
                    let columns = mysql::AllDBPrivs()
                        .into_iter()
                        .chain(std::iter::once(mysql::GrantPriv))
                        .filter(|privilege| static_mask.0 & privilege.0 != 0)
                        .map(|privilege| privilege.ColumnString().to_owned())
                        .collect::<Vec<_>>();
                    privileges.GrantDatabasePrivilegeColumns(&host, user, &database, &columns);
                }
                ast::GrantLevelType::Table => {
                    if static_mask != mysql::PrivilegeType(0) {
                        let table_names = mysql::AllTablePrivs()
                            .into_iter()
                            .chain(std::iter::once(mysql::GrantPriv))
                            .filter(|privilege| static_mask.0 & privilege.0 != 0)
                            .map(|privilege| privilege.SetString().to_owned())
                            .collect::<Vec<_>>();
                        let column_names = mysql::AllColumnPrivs()
                            .into_iter()
                            .filter(|privilege| static_mask.0 & privilege.0 != 0)
                            .map(|privilege| privilege.SetString().to_owned())
                            .collect::<Vec<_>>();
                        privileges.GrantTablePrivilegeMask(
                            &host,
                            user,
                            &database,
                            &table,
                            astersql_privilege_privileges::decodeSetToPrivilege(&table_names),
                            astersql_privilege_privileges::decodeSetToPrivilege(&column_names),
                        );
                    }
                    for (column, mask) in &column_grants {
                        let names = vec![mask.SetString().to_owned()];
                        privileges.GrantColumnPrivilegeMask(
                            &host,
                            user,
                            &database,
                            &table,
                            column,
                            astersql_privilege_privileges::decodeSetToPrivilege(&names),
                        );
                    }
                }
            }
            for privilege in &statement.Privs {
                if privilege.Name.is_empty() {
                    continue;
                }
                let name = privilege.Name.to_ascii_uppercase();
                if let Some(record) = privileges.dynamic_priv.iter_mut().find(|record| {
                    record.base.fullyMatch(user, &host)
                        && record.PrivilegeName.eq_ignore_ascii_case(&name)
                }) {
                    record.GrantOption |= statement.WithGrant;
                } else {
                    privileges.dynamic_priv.push(
                        astersql_privilege_privileges::dynamicPrivRecord {
                            base: astersql_privilege_privileges::baseRecord::new(&host, user),
                            PrivilegeName: name,
                            GrantOption: statement.WithGrant,
                        },
                    );
                }
            }
        }
        handle.merge(privileges);
        Ok(())
    }

    fn execute_privilege_mutation(&self, sql: &str) -> SessionResult<()> {
        let statements = parse(sql)?;
        let statement = statements
            .first()
            .ok_or_else(|| SessionError::new("privilege persistence SQL did not parse"))?;
        if let Some(insert) = statement.as_any().downcast_ref::<ast::InsertStmt>() {
            self.execute_insert(insert)
        } else if let Some(update) = statement.as_any().downcast_ref::<ast::UpdateStmt>() {
            self.execute_update(update)
        } else if let Some(delete) = statement.as_any().downcast_ref::<ast::DeleteStmt>() {
            self.execute_delete(delete)
        } else {
            Err(SessionError::new(
                "privilege persistence requires INSERT, UPDATE, or DELETE",
            ))
        }
    }

    pub(super) fn execute_drop_user(&self, statement: &ast::DropUserStmt) -> SessionResult<()> {
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"));
        let mut identities = Vec::new();
        for identity in &statement.UserList {
            let user = &identity.username;
            let host = account_host(&identity.hostname).to_ascii_lowercase();
            let exists = privileges
                .user
                .iter()
                .any(|record| record.base.fullyMatch(user, &host));
            if !exists && !statement.IfExists {
                return Err(SessionError::new(format!(
                    "Operation DROP USER failed for '{user}'@'{host}'"
                )));
            }
            identities.push((host, user.clone()));
        }

        if !identities.is_empty() {
            self.ensure_implicit_transaction()?;
            let result = (|| {
                for (host, user) in &identities {
                    for table in ["db", "tables_priv", "columns_priv", "user"] {
                        self.execute_privilege_mutation(&format!(
                            "DELETE FROM mysql.{table} WHERE User={} AND Host={}",
                            quote(user),
                            quote(host),
                        ))?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.finish_transaction(false)?;
                return Err(error);
            }
            self.finish_transaction(true)?;
        }

        for (host, user) in identities {
            privileges.DropAccount(&host, &user);
        }
        handle.merge(privileges);
        Ok(())
    }

    pub(super) fn execute_revoke(&self, statement: &ast::RevokeStmt) -> SessionResult<()> {
        use astersql_parser_mysql::privs as mysql;
        let handle = runtime_privilege_handle(&self.domain);
        let mut privileges = handle.Get();
        let database = if statement.Level.DBName.is_empty() {
            self.current_database()
        } else {
            statement.Level.DBName.clone()
        };
        if statement.Level.Level != ast::GrantLevelType::Global && database.is_empty() {
            return Err(SessionError::new("No database selected for REVOKE"));
        }
        let table = statement.Level.TableName.clone();
        let quote = |value: &str| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"));
        let user_columns = mysql::Priv2UserCol();
        let allowed = match statement.Level.Level {
            ast::GrantLevelType::Global => mysql::AllGlobalPrivs(),
            ast::GrantLevelType::DB => mysql::AllDBPrivs(),
            ast::GrantLevelType::Table => mysql::AllTablePrivs(),
        };
        let mut static_mask = mysql::PrivilegeType(0);
        let mut column_revokes = Vec::new();
        for privilege in &statement.Privs {
            if !privilege.Name.is_empty() {
                if statement.Level.Level != ast::GrantLevelType::Global {
                    return Err(SessionError::new("dynamic privileges require global scope"));
                }
                continue;
            }
            let revoked = if privilege.Priv == mysql::AllPriv {
                allowed.clone()
            } else if privilege.Priv == mysql::UsagePriv {
                Vec::new()
            } else if privilege.Cols.is_empty() && allowed.contains(&privilege.Priv) {
                vec![privilege.Priv]
            } else if statement.Level.Level == ast::GrantLevelType::Table
                && mysql::AllColumnPrivs().contains(&privilege.Priv)
                && !privilege.Cols.is_empty()
            {
                for column in &privilege.Cols {
                    column_revokes.push((column.Name.O.clone(), privilege.Priv));
                }
                Vec::new()
            } else {
                return Err(SessionError::new("illegal privilege level for REVOKE"));
            };
            for privilege in revoked {
                static_mask = mysql::PrivilegeType(static_mask.0 | privilege.0);
            }
        }
        let cache_static_mask = match statement.Level.Level {
            ast::GrantLevelType::Global | ast::GrantLevelType::DB => {
                let columns = allowed
                    .iter()
                    .copied()
                    .chain(std::iter::once(mysql::GrantPriv))
                    .filter(|privilege| static_mask.0 & privilege.0 != 0)
                    .map(|privilege| privilege.ColumnString().to_owned())
                    .collect::<Vec<_>>();
                astersql_privilege_privileges::DecodePrivilegeColumns(&columns)
            }
            ast::GrantLevelType::Table => {
                let names = mysql::AllTablePrivs()
                    .into_iter()
                    .chain(std::iter::once(mysql::GrantPriv))
                    .filter(|privilege| static_mask.0 & privilege.0 != 0)
                    .map(|privilege| privilege.SetString().to_owned())
                    .collect::<Vec<_>>();
                astersql_privilege_privileges::decodeSetToPrivilege(&names)
            }
        };

        for spec in &statement.Users {
            let user = &spec.User.username;
            let host = account_host(&spec.User.hostname).to_ascii_lowercase();
            if !privileges
                .user
                .iter()
                .any(|record| record.base.fullyMatch(user, &host))
            {
                return Err(SessionError::new(format!(
                    "Unknown user: '{user}'@'{host}'"
                )));
            }
        }

        self.ensure_implicit_transaction()?;
        let result = (|| {
            for spec in &statement.Users {
                let user = &spec.User.username;
                let host = account_host(&spec.User.hostname).to_ascii_lowercase();
                match statement.Level.Level {
                    ast::GrantLevelType::Global => {
                        let assignments = mysql::AllGlobalPrivs()
                            .into_iter()
                            .chain(std::iter::once(mysql::GrantPriv))
                            .filter(|privilege| static_mask.0 & privilege.0 != 0)
                            .filter_map(|privilege| user_columns.get(&privilege))
                            .map(|column| format!("{column}='N'"))
                            .collect::<Vec<_>>();
                        if !assignments.is_empty() {
                            let columns = assignments
                                .iter()
                                .map(|assignment| assignment.split('=').next().unwrap_or_default())
                                .collect::<Vec<_>>();
                            self.execute_privilege_mutation(&format!(
                                "INSERT INTO mysql.user (Host, User, {}) VALUES ({}, {}, {}) ON DUPLICATE KEY UPDATE {}",
                                columns.join(", "), quote(&host), quote(user),
                                vec!["'N'"; columns.len()].join(", "), assignments.join(", ")
                            ))?;
                        }
                        for privilege in &statement.Privs {
                            if !privilege.Name.is_empty() {
                                self.execute_privilege_mutation(&format!(
                                    "DELETE FROM mysql.global_grants WHERE User={} AND Host={} AND Priv={}",
                                    quote(user), quote(&host), quote(&privilege.Name.to_ascii_uppercase())
                                ))?;
                            }
                        }
                        if statement
                            .Privs
                            .iter()
                            .any(|privilege| privilege.Priv == mysql::AllPriv)
                        {
                            self.execute_privilege_mutation(&format!(
                                "DELETE FROM mysql.global_grants WHERE User={} AND Host={}",
                                quote(user),
                                quote(&host)
                            ))?;
                        }
                    }
                    ast::GrantLevelType::DB => {
                        let assignments = mysql::AllDBPrivs()
                            .into_iter()
                            .chain(std::iter::once(mysql::GrantPriv))
                            .filter(|privilege| static_mask.0 & privilege.0 != 0)
                            .filter_map(|privilege| user_columns.get(&privilege))
                            .map(|column| format!("{column}='N'"))
                            .collect::<Vec<_>>();
                        if !assignments.is_empty() {
                            let columns = assignments
                                .iter()
                                .map(|assignment| assignment.split('=').next().unwrap_or_default())
                                .collect::<Vec<_>>();
                            self.execute_privilege_mutation(&format!(
                                "INSERT INTO mysql.db (Host, DB, User, {}) VALUES ({}, {}, {}, {}) ON DUPLICATE KEY UPDATE {}",
                                columns.join(", "), quote(&host), quote(&database), quote(user),
                                vec!["'N'"; columns.len()].join(", "), assignments.join(", ")
                            ))?;
                        }
                        let remaining = privileges
                            .db
                            .iter()
                            .find(|record| {
                                record.base.fullyMatch(user, &host)
                                    && record.DB.eq_ignore_ascii_case(&database)
                            })
                            .map_or(0, |record| record.Privileges & !cache_static_mask);
                        if remaining == 0 {
                            self.execute_privilege_mutation(&format!(
                                "DELETE FROM mysql.db WHERE User={} AND Host={} AND DB={}",
                                quote(user),
                                quote(&host),
                                quote(&database)
                            ))?;
                        }
                    }
                    ast::GrantLevelType::Table => {
                        if static_mask != mysql::PrivilegeType(0) {
                            let remaining = privileges
                                .tables_priv
                                .iter()
                                .find(|record| {
                                    record.base.fullyMatch(user, &host)
                                        && record.DB.eq_ignore_ascii_case(&database)
                                        && record.TableName.eq_ignore_ascii_case(&table)
                                })
                                .map(|record| {
                                    (
                                        record.TablePriv & !cache_static_mask,
                                        record.ColumnPriv & !cache_static_mask,
                                    )
                                })
                                .unwrap_or((0, 0));
                            if remaining == (0, 0) {
                                self.execute_privilege_mutation(&format!(
                                    "DELETE FROM mysql.tables_priv WHERE User={} AND Host={} AND DB={} AND Table_name={}", quote(user), quote(&host), quote(&database), quote(&table)
                                ))?;
                            } else {
                                let mut table_names =
                                    astersql_privilege_privileges::EncodePrivilegeSet(
                                        remaining.0,
                                        astersql_privilege_privileges::ALL_TABLE_PRIVS,
                                    );
                                if remaining.0 & astersql_privilege_privileges::GrantPriv != 0 {
                                    table_names.push("Grant".to_owned());
                                }
                                let column_names =
                                    astersql_privilege_privileges::EncodePrivilegeSet(
                                        remaining.1,
                                        astersql_privilege_privileges::ALL_TABLE_PRIVS,
                                    );
                                self.execute_privilege_mutation(&format!(
                                    "INSERT INTO mysql.tables_priv (Host, DB, User, Table_name, Table_priv, Column_priv) VALUES ({}, {}, {}, {}, {}, {}) ON DUPLICATE KEY UPDATE Table_priv={}, Column_priv={}",
                                    quote(&host), quote(&database), quote(user), quote(&table),
                                    quote(&table_names.join(",")), quote(&column_names.join(",")),
                                    quote(&table_names.join(",")), quote(&column_names.join(","))
                                ))?;
                            }
                        }
                        for (column, mask) in &column_revokes {
                            let cache_column_mask =
                                astersql_privilege_privileges::decodeSetToPrivilege(&[mask
                                    .SetString()
                                    .to_owned()]);
                            let remaining = privileges
                                .columns_priv
                                .iter()
                                .find(|record| {
                                    record.base.fullyMatch(user, &host)
                                        && record.DB.eq_ignore_ascii_case(&database)
                                        && record.TableName.eq_ignore_ascii_case(&table)
                                        && record.ColumnName.eq_ignore_ascii_case(column)
                                })
                                .map_or(0, |record| record.ColumnPriv & !cache_column_mask);
                            if remaining == 0 {
                                self.execute_privilege_mutation(&format!(
                                    "DELETE FROM mysql.columns_priv WHERE User={} AND Host={} AND DB={} AND Table_name={} AND Column_name={}",
                                    quote(user), quote(&host), quote(&database), quote(&table), quote(column)
                                ))?;
                            } else {
                                let value = astersql_privilege_privileges::EncodePrivilegeSet(
                                    remaining,
                                    astersql_privilege_privileges::ALL_TABLE_PRIVS,
                                )
                                .join(",");
                                self.execute_privilege_mutation(&format!(
                                    "INSERT INTO mysql.columns_priv (Host, DB, User, Table_name, Column_name, Column_priv) VALUES ({}, {}, {}, {}, {}, {}) ON DUPLICATE KEY UPDATE Column_priv={}",
                                    quote(&host), quote(&database), quote(user), quote(&table), quote(column), quote(&value), quote(&value)
                                ))?;
                            }
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.finish_transaction(false)?;
            return Err(error);
        }
        self.finish_transaction(true)?;

        for spec in &statement.Users {
            let user = &spec.User.username;
            let host = account_host(&spec.User.hostname).to_ascii_lowercase();
            match statement.Level.Level {
                ast::GrantLevelType::Global => {
                    privileges.RevokePrivilegeMask(&host, user, None, None, None, cache_static_mask)
                }
                ast::GrantLevelType::DB => privileges.RevokePrivilegeMask(
                    &host,
                    user,
                    Some(&database),
                    None,
                    None,
                    cache_static_mask,
                ),
                ast::GrantLevelType::Table => {
                    privileges.RevokePrivilegeMask(
                        &host,
                        user,
                        Some(&database),
                        Some(&table),
                        None,
                        cache_static_mask,
                    );
                    for (column, mask) in &column_revokes {
                        let names = vec![mask.SetString().to_owned()];
                        privileges.RevokePrivilegeMask(
                            &host,
                            user,
                            Some(&database),
                            Some(&table),
                            Some(column),
                            astersql_privilege_privileges::decodeSetToPrivilege(&names),
                        );
                    }
                }
            }
            for privilege in &statement.Privs {
                if !privilege.Name.is_empty() {
                    privileges.dynamic_priv.retain(|record| {
                        !(record.base.fullyMatch(user, &host)
                            && record.PrivilegeName.eq_ignore_ascii_case(&privilege.Name))
                    });
                }
            }
            if statement.Level.Level == ast::GrantLevelType::Global
                && statement
                    .Privs
                    .iter()
                    .any(|privilege| privilege.Priv == mysql::AllPriv)
            {
                privileges
                    .dynamic_priv
                    .retain(|record| !record.base.fullyMatch(user, &host));
            }
        }
        handle.merge(privileges);
        Ok(())
    }

    pub(super) fn resource_group_priority(&self, name: &str) -> ArbitrationPriority {
        RUNTIME_RESOURCE_GROUPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&runtime_domain_id(&self.domain))
            .and_then(|groups| groups.get(&name.to_ascii_lowercase()))
            .map_or(ArbitrationPriorityMedium, |group| group.priority)
    }

    #[cfg(test)]
    pub(crate) fn compiler_quota_at_executor_build_for_test(&self) -> i64 {
        self.compiler_quota_at_executor_build
            .load(Ordering::Acquire)
    }

    pub(crate) fn begin_compile_memory_arbitration(
        &self,
        normalized_sql: &str,
        has_select: bool,
    ) -> SessionResult<Option<RuntimeCompileMemoryQuota>> {
        let Some(arbitrator) = GlobalMemArbitrator() else {
            return Ok(None);
        };
        if self.connection_id() == 0 || self.state.borrow().mem_arbitrator_wait_averse == "nolimit"
        {
            return Ok(None);
        }
        let reserved =
            super::approx_compile_plan_token_count(normalized_sql, has_select) * (63091 * 12 / 10);
        if reserved <= 0 {
            return Ok(None);
        }
        if arbitrator.AtMemRisk() {
            self.clear_memory_sensitive_plan_cache();
            let mut delay = Duration::from_millis(100);
            while arbitrator.AtMemRisk() {
                if arbitrator.AtOOMRisk() {
                    unsafe {
                        let tasks = &*std::ptr::addr_of!(
                            astersql_metrics::memory::GlobalMemArbitratorSubTasks
                        );
                        if let Some(counter) = &tasks.ForceKillPlan {
                            counter.inc();
                        }
                    }
                    return Err(SessionError::new(format!(
                        "[executor:8180]Query execution was stopped by the global memory arbitrator [reason={}, path=CompilePlan] [conn={}]",
                        astersql_util_memory::ArbitratorOOMRiskKill.String(),
                        self.connection_id()
                    )));
                }
                self.sql_killer
                    .HandleSignal()
                    .map_err(|error| SessionError::new(error.to_string()))?;
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(1));
            }
        }
        let ok = arbitrator.ConsumeQuotaFromAwaitFreePool(self.connection_id(), reserved);
        let quota = RuntimeCompileMemoryQuota {
            arbitrator,
            uid: self.connection_id(),
            reserved,
        };
        if !ok {
            self.clear_memory_sensitive_plan_cache();
        }
        // A false allocation result still charges the await-free budget. The
        // guard releases exactly that charge, including compiler errors/panic.
        Ok(Some(quota))
    }

    fn clear_memory_sensitive_plan_cache(&self) {
        self.instance_plan_cache.Evict(true);
        let mut state = self.state.borrow_mut();
        state.non_prepared_plan_cache_keys.clear();
        for prepared in state.prepared_by_name.values_mut() {
            prepared.planned = false;
            prepared.cached_transaction_contexts.clear();
            prepared.typed_plan_id = None;
            prepared.typed_plan_catalog_version = None;
        }
    }

    pub(super) fn init_statement_memory_tracker(
        &self,
        tracker: &mut Tracker,
        normalized_sql: &str,
        hints: &astersql_util_hint::StmtHints,
    ) -> SessionResult<()> {
        let Some(arbitrator) = GlobalMemArbitrator() else {
            return Ok(());
        };
        let state = self.state.borrow();
        if self.connection_id() == 0 || state.mem_arbitrator_wait_averse == "nolimit" {
            return Ok(());
        }
        let reserved = hints
            .SetVars
            .get(astersql_sessionctx_vardef::TiDBMemArbitratorQueryReserved)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(state.mem_arbitrator_query_reserved)
            .min(astersql_util_memory::DefMaxLimit);
        let wait_averse = arbitrator.WorkMode() == astersql_util_memory::ArbitratorModePriority
            && state.mem_arbitrator_wait_averse == "1";
        drop(state);
        let group = if hints.HasResourceGroup {
            hints.ResourceGroup.as_str()
        } else {
            "default"
        };
        tracker.SessionID.Store(self.connection_id());
        if !tracker.InitMemArbitratorWithSharedKiller(
            Some(arbitrator),
            Some(self.sql_killer.clone()),
            super::build_mem_arbitrator_digest_id(normalized_sql, &self.current_database()),
            self.resource_group_priority(group),
            wait_averse,
            reserved,
            self.session_vars.InRestrictedSQL,
        ) {
            return Err(SessionError::new("failed to init mem-arbitrator"));
        }
        self.sql_killer
            .HandleSignal()
            .map_err(|error| SessionError::new(error.to_string()))
    }

    pub(super) fn begin_statement_memory_arbitration(
        &self,
        statement: &dyn ast::Node,
        hints: &astersql_util_hint::StmtHints,
        sql: &str,
        tracker: &mut Tracker,
    ) -> SessionResult<Option<RuntimeMemoryArbitrationGuard>> {
        let sensitive = statement.as_any().is::<ast::SelectStmt>()
            || statement.as_any().is::<ast::InsertStmt>()
            || statement.as_any().is::<ast::UpdateStmt>()
            || statement.as_any().is::<ast::DeleteStmt>();
        if !sensitive {
            return Ok(None);
        }
        let has_select = statement.as_any().is::<ast::SelectStmt>();
        let normalized_sql = astersql_parser::NormalizeDigest(sql).0;
        if super::approx_compile_plan_token_count(&normalized_sql, has_select) == 0 {
            return Ok(None);
        }
        let reserved = hints
            .SetVars
            .get(astersql_sessionctx_vardef::TiDBMemArbitratorQueryReserved)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(self.state.borrow().mem_arbitrator_query_reserved);
        self.state
            .borrow_mut()
            .statement_mem_arbitrator_query_reserved = Some(reserved);
        // Start a fresh statement kill event, as the previous arbitrator helper
        // did before constructing its context. A completed cancellation must
        // not suppress the next statement's arbitration request.
        self.sql_killer.Reset();
        self.init_statement_memory_tracker(tracker, &normalized_sql, hints)?;
        Ok(GlobalMemArbitrator()
            .filter(|_| tracker.MemArbitrator.is_some())
            .map(|arbitrator| RuntimeMemoryArbitrationGuard {
                arbitrator,
                uid: tracker.SessionID.Load(),
            }))
    }

    /// 执行 SET 语句（系统/用户变量）。
    pub(super) fn execute_set(&self, statement: &ast::SetStmt) -> SessionResult<()> {
        for variable in &statement.Variables {
            let raw_name = variable.Name.to_lowercase();
            let is_global = variable.IsGlobal || raw_name.starts_with("@@global.");
            let name = ["@@global.", "@@instance.", "@@session.", "@@local.", "@@"]
                .iter()
                .find_map(|prefix| raw_name.strip_prefix(prefix))
                .unwrap_or(&raw_name)
                .to_owned();
            // Go validates system-variable scope before evaluating the value.
            // `tidb_current_ts` has ScopeNone/ReadOnly metadata, so every SET
            // form must fail without evaluating the right-hand expression.
            if name == "tidb_current_ts" {
                return Err(SessionError::new("Variable 'tidb_current_ts' is read only"));
            }
            let is_user_variable = !variable.IsSystem
                && !raw_name.starts_with("@@")
                && !matches!(name.as_str(), "tx_isolation" | "transaction_isolation");
            let value = if is_user_variable
                && matches!(
                    &variable.Value.Kind,
                    ast::ExprKind::Value(value)
                        if matches!(value.Datum, ast::ValueDatum::Null)
                ) {
                "null".to_owned()
            } else if variable.Value.IsDefaultExpr()
                && matches!(name.as_str(), "tidb_enable_foreign_key" | "timestamp")
            {
                if name == "timestamp" {
                    "0".to_owned()
                } else {
                    "1".to_owned()
                }
            } else if variable.Value.IsDefaultExpr() {
                let system_variable =
                    astersql_sessionctx_variable::GetSysVar(&name).ok_or_else(|| {
                        SessionError::new(format!("Unknown system variable '{}'", variable.Name))
                    })?;
                astersql_sessionctx_variable::sysvar::GlobalSystemVariableInitialValue(
                    &system_variable.Name,
                    &system_variable.Value,
                )
            } else if name == "tidb_snapshot"
                && matches!(
                    &variable.Value.Kind,
                    ast::ExprKind::Value(value)
                        if matches!(value.Datum, ast::ValueDatum::Null)
                )
            {
                String::new()
            } else if name == astersql_sessionctx_vardef::CharacterSetResults
                && matches!(
                    &variable.Value.Kind,
                    ast::ExprKind::Value(value)
                        if matches!(value.Datum, ast::ValueDatum::Null)
                )
            {
                // MySQL treats NULL here as “no result character-set
                // conversion”. Connector/J sends this during initialization.
                String::new()
            } else if name == "tx_read_ts" {
                // Evaluated below by the stale-read timestamp evaluator, which
                // supports NOW()/INTERVAL and bounded-staleness expressions.
                String::new()
            } else {
                self.evaluate_set_expression(&variable.Value)?
            };
            // Go `SET @name = value` writes a session user variable rather than
            // a system variable.
            if is_user_variable {
                let name = variable.Name.to_lowercase();
                let is_string = matches!(
                    &variable.Value.Kind,
                    ast::ExprKind::Value(value)
                        if matches!(
                            value.Datum,
                            ast::ValueDatum::String(_) | ast::ValueDatum::Bytes(_)
                        )
                );
                let mut state = self.state.borrow_mut();
                state.user_variables.insert(name.clone(), value);
                if is_string {
                    state.string_user_variables.insert(name);
                } else {
                    state.string_user_variables.remove(&name);
                }
                continue;
            }
            if name == astersql_sessionctx_vardef::TiDBEnableMDL {
                if !is_global {
                    return Err(SessionError::new(
                        "Variable 'tidb_enable_metadata_lock' is a GLOBAL variable and should be set with SET GLOBAL",
                    ));
                }
                if astersql_config_kerneltype::IsNextGen()
                    && astersql_sessionctx_vardef::IsReadOnlyVarInNextGen(&name)
                {
                    return Err(SessionError::new(
                        "Variable 'tidb_enable_metadata_lock' is read only in NextGen",
                    ));
                }
                let enabled = match value
                    .trim_matches(['\'', '"'])
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "1" | "on" | "true" => true,
                    "0" | "off" | "false" => false,
                    _ => {
                        return Err(SessionError::new(format!(
                            "Variable '{name}' can't be set to the value of '{value}'"
                        )));
                    }
                };
                self.domain
                    .set_stats_global_variable(&name, if enabled { "ON" } else { "OFF" })
                    .map_err(|error| session_error("persist global MDL setting", error))?;
                astersql_sessionctx_vardef::SetEnableMDL(enabled);
                astersql_ddl_schemaver::SetMDLEnabled(enabled);
                continue;
            }
            if matches!(
                name.as_str(),
                astersql_sessionctx_vardef::TiDBAnalyzeDefaultNumBuckets
                    | astersql_sessionctx_vardef::TiDBAnalyzeDefaultNumTopN
                    | astersql_sessionctx_vardef::TiDBPersistAnalyzeOptions
                    | astersql_sessionctx_vardef::TiDBStatsLoadPseudoTimeout
            ) {
                let metadata = ConcreteSession::new(Arc::clone(&self.domain));
                metadata.SetInRestrictedSQL(true);
                use astersql_sessionctx_vardef as vardef;
                let (normalized, warnings) = self
                    .session_vars
                    .ValidateAndSetGlobalSystemVar(
                        &name,
                        value.trim_matches(['\'', '"']),
                        if is_global {
                            vardef::ScopeGlobal
                        } else {
                            vardef::ScopeSession
                        },
                    )
                    .map_err(|error| SessionError::new(error.to_string()))?;
                for warning in warnings {
                    self.set_warning(warning.to_string());
                }
                self.domain.set_global_system_variable(&name, &normalized);
                metadata.execute(&format!("INSERT INTO mysql.global_variables (variable_name,variable_value) VALUES ('{name}','{normalized}') ON DUPLICATE KEY UPDATE variable_value='{normalized}'"))?;
                continue;
            }
            if astersql_sessionctx_variable::is_embedding_api_key(&name)
                || name == astersql_sessionctx_variable::EMBEDDING_API_BASE
            {
                if !is_global {
                    return Err(SessionError::new(format!(
                        "Variable '{name}' is a GLOBAL variable and should be set with SET GLOBAL"
                    )));
                }
                let (raw, warnings) = self
                    .session_vars
                    .ValidateAndSetGlobalSystemVar(
                        &name,
                        value.trim_matches(['\'', '"']),
                        astersql_sessionctx_vardef::ScopeGlobal,
                    )
                    .map_err(|error| SessionError::new(error.to_string()))?;
                for warning in warnings {
                    self.set_warning(warning.to_string());
                }
                let normalized = if name == astersql_sessionctx_variable::EMBEDDING_API_BASE {
                    astersql_sessionctx_variable::NormalizeOpenAIEmbeddingAPIBase(&raw)
                        .map_err(SessionError::new)?
                } else {
                    raw
                };
                let metadata = ConcreteSession::new(Arc::clone(&self.domain));
                metadata.SetInRestrictedSQL(true);
                let persisted = normalized.replace('\\', "\\\\").replace('\'', "''");
                metadata.execute(&format!("INSERT INTO mysql.global_variables (variable_name,variable_value) VALUES ('{name}','{persisted}') ON DUPLICATE KEY UPDATE variable_value='{persisted}'"))?;
                self.domain.set_global_system_variable(&name, &normalized);
                continue;
            }
            if name == astersql_sessionctx_vardef::TiDBGCLifetime {
                if !is_global {
                    return Err(SessionError::new(
                        "Variable 'tidb_gc_life_time' is a GLOBAL variable and should be set with SET GLOBAL",
                    ));
                }
                let nanos = astersql_sessionctx_variable::parse_go_duration(
                    value.trim_matches(['\'', '"']),
                )
                .ok_or_else(|| {
                    SessionError::new(format!("Incorrect argument type to variable '{name}'"))
                })?;
                let bounded = nanos.clamp(600_000_000_000, 31_536_000_000_000_000);
                let normalized = astersql_sessionctx_variable::format_go_duration(bounded);
                // GC's hook stores the effective lifetime in mysql.tidb, not the generic global-variable cache.
                let metadata = ConcreteSession::new(self.domain.clone());
                metadata.execute(&format!("INSERT INTO mysql.tidb (VARIABLE_NAME,VARIABLE_VALUE,COMMENT) VALUES ('tikv_gc_life_time','{normalized}','All versions within life time will not be collected by GC, at least 10m, in Go format.') ON DUPLICATE KEY UPDATE VARIABLE_VALUE='{normalized}'"))?;
                self.domain.set_global_system_variable(&name, &normalized);
                if bounded != nanos {
                    self.set_warning(format!(
                        "Truncated incorrect tidb_gc_life_time value: '{value}'"
                    ));
                }
                super::session::notify_external_workload_gc_lifetime(&self.domain);
                continue;
            }
            if name == astersql_sessionctx_vardef::TiDBServiceScope {
                let (normalized, warnings) = self
                    .session_vars
                    .ValidateAndSetGlobalSystemVar(
                        &name,
                        value.trim_matches(['\'', '"']),
                        if is_global || raw_name.starts_with("@@instance.") {
                            astersql_sessionctx_vardef::ScopeGlobal
                        } else {
                            astersql_sessionctx_vardef::ScopeSession
                        },
                    )
                    .map_err(|error| session_error("set DDL service scope", error))?;
                for warning in warnings {
                    self.set_warning(warning.to_string());
                }
                self.domain.set_global_system_variable(&name, &normalized);
                continue;
            }
            if name == astersql_sessionctx_vardef::TiDBPagingSizeBytes {
                let (normalized, warnings) = self
                    .session_vars
                    .ValidateAndSetGlobalSystemVar(
                        &name,
                        value.trim_matches(['\'', '"']),
                        if is_global {
                            astersql_sessionctx_vardef::ScopeGlobal
                        } else {
                            astersql_sessionctx_vardef::ScopeSession
                        },
                    )
                    .map_err(|error| SessionError::new(error.to_string()))?;
                for warning in warnings {
                    self.set_warning(warning.to_string());
                }
                self.domain.set_global_system_variable(&name, &normalized);
                continue;
            }
            if self.execute_mem_arbitrator_set(is_global, &name, &value)? {
                continue;
            }
            if is_global && name == astersql_sessionctx_vardef::TiDBTraceEvent {
                astersql_sessionctx_variable::sysvar_builtins::SetTraceEventConfig(&value)
                    .map_err(|error| SessionError::new(error))?;
                continue;
            }
            if is_global && name == astersql_sessionctx_vardef::TiDBDDLErrorCountLimit {
                let raw = value.trim_matches(['\'', '"']);
                let parsed = raw.parse::<i128>().map_err(|_| {
                    SessionError::new(format!("Incorrect argument type to variable '{name}'"))
                })?;
                let clamped = parsed.clamp(0, i64::MAX as i128) as i64;
                if parsed != clamped as i128 {
                    self.state.borrow_mut().current_warnings.push(
                        SessionWarning::warning_with_code(
                            1292,
                            format!("Truncated incorrect {name} value: '{raw}'"),
                        ),
                    );
                }
                astersql_sessionctx_vardef::SetDDLErrorCountLimit(clamped);
                self.domain
                    .set_stats_global_variable(&name, &clamped.to_string())
                    .map_err(|error| session_error("set global DDL variable", error))?;
                continue;
            }
            if is_global && name == astersql_sessionctx_vardef::TiDBDDLReorgMaxWriteSpeed {
                let (normalized, warnings) = self
                    .session_vars
                    .ValidateAndSetGlobalSystemVar(
                        &name,
                        value.trim_matches(['\'', '"']),
                        astersql_sessionctx_vardef::ScopeGlobal,
                    )
                    .map_err(|error| session_error("set global DDL write speed", error))?;
                for warning in warnings {
                    self.set_warning(warning.to_string());
                }
                self.domain
                    .set_stats_global_variable(&name, &normalized)
                    .map_err(|error| session_error("set global DDL variable", error))?;
                continue;
            }
            if is_global && name == astersql_sessionctx_vardef::TiDBEnableDistTask {
                let raw = value.trim_matches(['\'', '"']);
                let enabled = match raw.to_ascii_lowercase().as_str() {
                    "1" | "on" | "true" => true,
                    "0" | "off" | "false" => false,
                    _ => {
                        return Err(SessionError::new(format!(
                            "Variable '{name}' can't be set to the value of '{raw}'"
                        )));
                    }
                };
                astersql_sessionctx_vardef::EnableDistTask.Store(enabled);
                self.state.borrow_mut().dist_task_enabled = enabled;
                self.domain
                    .set_stats_global_variable(&name, if enabled { "ON" } else { "OFF" })
                    .map_err(|error| session_error("set global DDL variable", error))?;
                continue;
            }
            if is_global
                && name == astersql_sessionctx_vardef::TiDBInstancePlanCacheReservedPercentage
            {
                let raw = value.trim_matches(['\'', '"']);
                let parsed = raw.parse::<f64>().map_err(|_| {
                    SessionError::new(
                        "tidb_instance_plan_cache_reserved_percentage must be a number",
                    )
                })?;
                let percentage = parsed.clamp(0.0, 1.0);
                if percentage != parsed {
                    self.state.borrow_mut().current_warnings.push(
                        SessionWarning::warning_with_code(
                            1292,
                            format!("Truncated incorrect {name} value: '{raw}'"),
                        ),
                    );
                }
                let normalized = percentage.to_string();
                self.domain
                    .set_stats_global_variable(&name, &normalized)
                    .map_err(|error| session_error("set global statistics variable", error))?;
                astersql_sessionctx_vardef::InstancePlanCacheReservedPercentage.Store(percentage);
                continue;
            }
            if name == astersql_sessionctx_vardef::TiDBMergePartitionStatsConcurrency {
                let (normalized, warnings) = self
                    .session_vars
                    .ValidateAndSetGlobalSystemVar(
                        &name,
                        value.trim_matches(['\'', '"']),
                        if is_global {
                            astersql_sessionctx_vardef::ScopeGlobal
                        } else {
                            astersql_sessionctx_vardef::ScopeSession
                        },
                    )
                    .map_err(|error| SessionError::new(error.to_string()))?;
                for warning in warnings {
                    let code = if warning.kind()
                        == astersql_sessionctx_variable::VariableErrorKind::TruncatedWrongValue
                    {
                        1292
                    } else {
                        1287
                    };
                    self.state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning::warning_with_code(code, warning.to_string()));
                }
                if is_global {
                    self.domain
                        .set_stats_global_variable(&name, &normalized)
                        .map_err(|error| session_error("set deprecated merge variable", error))?;
                }
                continue;
            }
            if variable.IsGlobal {
                // Go surfaces sysvar `Validation` failures verbatim, so keep the
                // statistics variable message unwrapped.
                self.domain
                    .set_stats_global_variable(&name, &value)
                    .map_err(|error| match error {
                        astersql_domain::DomainError::Stats(message) => SessionError::new(message),
                        error => session_error("set global statistics variable", error),
                    })?;
                if name == astersql_sessionctx_vardef::TiDBMemOOMAction {
                    self.state.borrow_mut().global_mem_oom_action = value.clone();
                }
            }
            match name.as_str() {
                astersql_sessionctx_vardef::TiDBSlowLogThreshold => {
                    self.state.borrow_mut().slow_log_threshold_ms = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse tidb_slow_log_threshold", error))?;
                }
                astersql_sessionctx_vardef::TiDBEnableInstancePlanCache => {
                    if !variable.IsGlobal {
                        return Err(SessionError::new(
                            "tidb_enable_instance_plan_cache is a GLOBAL variable",
                        ));
                    }
                    astersql_sessionctx_vardef::EnableInstancePlanCache
                        .Store(variable_is_on(&value));
                }
                astersql_sessionctx_vardef::TiDBGeneralLog => {
                    astersql_sessionctx_vardef::ProcessGeneralLog.Store(variable_is_on(&value));
                }
                astersql_sessionctx_vardef::TiDBInstancePlanCacheReservedPercentage => {
                    if !variable.IsGlobal {
                        return Err(SessionError::new(
                            "tidb_instance_plan_cache_reserved_percentage is a GLOBAL variable",
                        ));
                    }
                    let percentage =
                        value
                            .trim_matches(['\'', '"'])
                            .parse::<f64>()
                            .map_err(|_| {
                                SessionError::new(
                                    "tidb_instance_plan_cache_reserved_percentage must be a number",
                                )
                            })?;
                    if !(0.0..=1.0).contains(&percentage) {
                        return Err(SessionError::new(format!(
                            "Variable '{}' can't be set to the value of '{value}'",
                            astersql_sessionctx_vardef::TiDBInstancePlanCacheReservedPercentage
                        )));
                    }
                    astersql_sessionctx_vardef::InstancePlanCacheReservedPercentage
                        .Store(percentage);
                }
                astersql_sessionctx_vardef::TiDBInstancePlanCacheMaxMemSize => {
                    if !variable.IsGlobal {
                        return Err(SessionError::new(
                            "tidb_instance_plan_cache_max_size is a GLOBAL variable",
                        ));
                    }
                    let value = value.trim_matches(['\'', '"']);
                    let (bytes, normalized) = astersql_sessionctx_variable::parseByteSize(value);
                    if normalized.is_empty()
                        || bytes
                            < astersql_sessionctx_vardef::MinTiDBInstancePlanCacheMemSize as u64
                    {
                        return Err(SessionError::new(format!(
                            "tidb_instance_plan_cache_max_size should be at least 100MiB"
                        )));
                    }
                    astersql_sessionctx_vardef::InstancePlanCacheMaxMemSize.Store(bytes as i64);
                }
                astersql_sessionctx_variable::vardef::TiDBRestrictedReadOnly => {
                    let enabled = variable_is_on(&value);
                    astersql_sessionctx_variable::vardef::RestrictedReadOnly.Store(enabled);
                    if enabled {
                        astersql_sessionctx_variable::vardef::VarTiDBSuperReadOnly.Store(true);
                    }
                }
                astersql_sessionctx_variable::vardef::TiDBSuperReadOnly => {
                    let enabled = variable_is_on(&value);
                    if !enabled && astersql_sessionctx_variable::vardef::RestrictedReadOnly.Load() {
                        return Err(SessionError::new(
                            "can't turn off tidb_super_read_only when \
                             tidb_restricted_read_only is on",
                        ));
                    }
                    astersql_sessionctx_variable::vardef::VarTiDBSuperReadOnly.Store(enabled);
                }
                "tidb_enable_historical_stats" => {
                    let enabled = matches!(value.to_lowercase().as_str(), "1" | "on" | "true");
                    self.domain.set_historical_stats_enabled(enabled);
                }
                "tidb_server_memory_limit" => {
                    if !variable.IsGlobal {
                        return Err(SessionError::new(
                            "tidb_server_memory_limit is a GLOBAL variable",
                        ));
                    }
                    let limit = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|_| {
                            SessionError::new("tidb_server_memory_limit must be an integer")
                        })?;
                    astersql_util_memory::tracker::ServerMemoryLimit.Store(limit);
                    SetGlobalMemArbitratorLimit(limit.min(i64::MAX as u64) as i64);
                }
                "tidb_snapshot" => {
                    let in_stale_transaction = {
                        let state = self.state.borrow();
                        state.transaction.is_some() && state.transaction_stale_read_ts.is_some()
                    };
                    if in_stale_transaction {
                        return Err(SessionError::new(
                            "Transaction characteristics can't be changed while a transaction is in progress",
                        ));
                    }
                    let (snapshot_read_ts, snapshot_catalog_version) = if value.is_empty()
                        || value.eq_ignore_ascii_case("null")
                    {
                        (None, None)
                    } else if let Ok(version) = value.parse::<u64>() {
                        let context = self.domain.stats_context();
                        let catalog_version =
                            self.state
                                .borrow()
                                .tso_catalog_versions
                                .range(..=version)
                                .next_back()
                                .map(|(_, catalog_version)| *catalog_version)
                                .or_else(|| context.has_catalog_version(version).then_some(version))
                                .or_else(|| {
                                    self.domain.snapshot_info_schema(version).ok().and_then(
                                        |schema| u64::try_from(schema.SchemaMetaVersion()).ok(),
                                    )
                                });
                        (Some(version), catalog_version)
                    } else {
                        let micros =
                            parse_stale_datetime_micros(value.trim_matches(['\'', '"']))
                                .map_err(|error| session_error("parse tidb_snapshot", error))?;
                        let snapshot_time = UNIX_EPOCH
                            + std::time::Duration::from_nanos(micros.max(0) as u64 * 1_000 + 999);
                        let catalog_version = self
                            .domain
                            .stats_context()
                            .catalog_version_at_time(snapshot_time)
                            .ok_or_else(|| {
                                SessionError::new("tidb_snapshot predates schema history")
                            })?;
                        (
                            Some(((micros.max(0) as u64) / 1_000) << 18),
                            Some(catalog_version),
                        )
                    };
                    let mut state = self.state.borrow_mut();
                    state.snapshot_catalog_version = snapshot_catalog_version;
                    state.snapshot_read_ts = snapshot_read_ts;
                    if snapshot_read_ts.is_some() {
                        state.pending_stale_read_ts = None;
                    }
                }
                "tx_read_ts" => {
                    if self.state.borrow().transaction.is_some() {
                        return Err(SessionError::new(
                            "set transaction read only as of is not allowed in a transaction",
                        ));
                    }
                    if literal(&variable.Value)
                        .is_ok_and(|value| value.trim_matches(['\'', '"']).is_empty())
                    {
                        let mut state = self.state.borrow_mut();
                        state.pending_stale_read_ts = None;
                        state.snapshot_catalog_version = None;
                        state.snapshot_read_ts = None;
                        continue;
                    }
                    let read_ts = self.evaluate_stale_read_ts(&variable.Value)?;
                    let mut state = self.state.borrow_mut();
                    state.pending_stale_read_ts = Some(read_ts);
                    state.snapshot_catalog_version = None;
                    state.snapshot_read_ts = None;
                }
                "tidb_read_staleness" => {
                    let text = value.trim_matches(['\'', '"']);
                    let seconds = if text.is_empty() {
                        None
                    } else {
                        Some(
                            text.parse::<i64>()
                                .ok()
                                .filter(|seconds| *seconds < 0)
                                .and_then(i64::checked_abs)
                                .and_then(|seconds| u64::try_from(seconds).ok())
                                .ok_or_else(|| {
                                    SessionError::new(
                                        "tidb_read_staleness must be a negative number of seconds",
                                    )
                                })?,
                        )
                    };
                    let read_ts = seconds.map(|seconds| {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        now.saturating_sub(seconds).saturating_mul(1_000) << 18
                    });
                    let mut state = self.state.borrow_mut();
                    state.session_read_staleness_seconds = seconds;
                    state.session_stale_read_ts = read_ts;
                }
                "tidb_enable_external_ts_read" => {
                    self.state.borrow_mut().enable_external_ts_read = variable_is_on(&value);
                }
                "tidb_external_ts" => {
                    self.state.borrow_mut().external_read_ts = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse tidb_external_ts", error))?;
                }
                "tidb_build_stats_concurrency" | "tidb_analyze_partition_concurrency" => {
                    let concurrency = value
                        .parse::<usize>()
                        .map_err(|error| session_error("parse analyze build concurrency", error))?;
                    if !(1..=256).contains(&concurrency) {
                        return Err(SessionError::new(
                            "analyze build concurrency must be between 1 and 256",
                        ));
                    }
                    self.state.borrow_mut().analyze_concurrency = concurrency;
                }
                "tidb_stats_load_sync_wait" => {
                    let wait = value
                        .trim_matches(['\'', '"'])
                        .parse::<i64>()
                        .map_err(|error| session_error("parse tidb_stats_load_sync_wait", error))?;
                    if wait < 0 || wait > i64::from(i32::MAX) {
                        return Err(SessionError::new(
                            "tidb_stats_load_sync_wait must be between 0 and 2147483647",
                        ));
                    }
                    self.state.borrow_mut().stats_load_sync_wait = wait;
                    self.session_vars
                        .StatsLoadSyncWait
                        .store(wait, Ordering::Release);
                }
                "tidb_opt_index_prune_threshold" => {
                    let threshold = value
                        .trim_matches(['\'', '"'])
                        .parse::<i64>()
                        .map_err(|error| session_error("parse index prune threshold", error))?;
                    if !(-1..=i64::from(i32::MAX)).contains(&threshold) {
                        return Err(SessionError::new(
                            "tidb_opt_index_prune_threshold must be between -1 and 2147483647",
                        ));
                    }
                    self.state.borrow_mut().opt_index_prune_threshold = threshold;
                }
                "sql_mode" => {
                    let formatted = astersql_parser_mysql::r#const::FormatSQLModeStr(
                        value.trim_matches(['\'', '"']),
                    );
                    astersql_parser_mysql::r#const::GetSQLMode(&formatted)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    self.state.borrow_mut().sql_mode = formatted;
                }
                astersql_sessionctx_vardef::CharacterSetServer
                | astersql_sessionctx_vardef::CollationServer => {
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| {
                            session_error("set server character system variable", error)
                        })?;
                }
                "time_zone" => {
                    let requested = value.trim_matches(['\'', '"']);
                    let time_zone = RuntimeTimeZone::parse(requested).ok_or_else(|| {
                        SessionError::new(format!("Unknown or incorrect time zone: {requested}"))
                    })?;
                    *self.time_zone.borrow_mut() = time_zone;
                    if !is_global {
                        // The runtime parser already validated named and fixed zones.
                        self.session_vars
                            .SetHintSystemVarWithRelaxedValidation(&name, requested)
                            .map_err(|error| {
                                session_error("set time zone system variable", error)
                            })?;
                    }
                }
                "auto_increment_increment" | "auto_increment_offset" => {
                    let parsed = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse auto-increment variable", error))?;
                    if !(1..=65_535).contains(&parsed) {
                        return Err(SessionError::new(format!(
                            "{name} must be between 1 and 65535"
                        )));
                    }
                    if name == "auto_increment_increment" {
                        self.state.borrow_mut().auto_increment_increment = parsed;
                    } else {
                        self.state.borrow_mut().auto_increment_offset = parsed;
                    }
                }
                "timestamp" => {
                    let value = value
                        .trim_matches(['\'', '"'])
                        .parse::<f64>()
                        .map_err(|error| session_error("parse timestamp", error))?;
                    self.state.borrow_mut().timestamp_override = (value != 0.0).then_some(value);
                }
                "max_connections" => {
                    self.state.borrow_mut().max_connections = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|_| SessionError::new("max_connections must be an integer"))?;
                }
                "max_prepared_stmt_count" => {
                    if !is_global {
                        return Err(SessionError::new(
                            "Variable 'max_prepared_stmt_count' is a GLOBAL variable",
                        ));
                    }
                    let value = value
                        .trim_matches(['\'', '"'])
                        .parse::<i64>()
                        .map_err(|_| {
                            SessionError::new(
                                "Incorrect argument type to variable 'max_prepared_stmt_count'",
                            )
                        })?;
                    if !(-1..=1_048_576).contains(&value) {
                        return Err(SessionError::new(
                            "Variable 'max_prepared_stmt_count' is out of range",
                        ));
                    }
                    runtime_set_max_prepared_stmt_count(&self.domain, value);
                }
                "max_user_connections" if value.trim_matches(['\'', '"']).is_empty() => {
                    return Err(SessionError::new(format!(
                        "Incorrect argument type to variable '{name}'"
                    )));
                }
                "tx_read_only" | "transaction_read_only" => {
                    let enabled = variable_is_on(&value);
                    let noop_enabled = if is_global {
                        self.state.borrow().global_enable_noop_functions
                    } else {
                        self.state.borrow().enable_noop_functions
                    };
                    if enabled && !noop_enabled {
                        return Err(SessionError::new(
                            "read only transaction is only supported when \
                             tidb_enable_noop_functions is enabled",
                        ));
                    }
                    if is_global {
                        self.state.borrow_mut().global_tx_read_only = enabled;
                    } else {
                        self.state.borrow_mut().tx_read_only = enabled;
                    }
                }
                "sql_require_primary_key" => {
                    self.state.borrow_mut().sql_require_primary_key = variable_is_on(&value);
                }
                astersql_sessionctx_vardef::CharacterSetResults => {
                    self.state.borrow_mut().character_set_results =
                        value.trim_matches(['\'', '"']).to_owned();
                }
                "tidb_analyze_version" => {
                    self.state.borrow_mut().analyze_version = value
                        .trim_matches(['\'', '"'])
                        .parse::<i32>()
                        .map_err(|error| session_error("parse tidb_analyze_version", error))?;
                }
                "tidb_mem_quota_analyze" => {
                    self.state.borrow_mut().analyze_memory_quota = value
                        .trim_matches(['\'', '"'])
                        .parse::<i64>()
                        .map_err(|error| session_error("parse tidb_mem_quota_analyze", error))?;
                }
                "tidb_partition_prune_mode" => {
                    self.state.borrow_mut().dynamic_partition_prune = match value
                        .trim_matches(['\'', '"'])
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "dynamic" | "dynamic-only" | "static-but-prepare-dynamic" => true,
                        "static" => false,
                        _ => {
                            return Err(SessionError::new(
                                "tidb_partition_prune_mode must be dynamic or static",
                            ));
                        }
                    };
                }
                "tidb_isolation_read_engines" => {
                    self.state.borrow_mut().isolation_read_engines =
                        value.trim_matches(['\'', '"']).to_ascii_lowercase();
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| session_error("set planner isolation engines", error))?;
                }
                "tiflash_fastscan" => {
                    self.state.borrow_mut().tiflash_fastscan = variable_is_on(&value);
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| session_error("set planner TiFlash fast scan", error))?;
                }
                "tidb_allow_tiflash_cop" => {
                    self.state.borrow_mut().allow_tiflash_cop = variable_is_on(&value);
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| {
                            session_error("set planner TiFlash Cop allowance", error)
                        })?;
                }
                "tidb_allow_mpp" => {
                    let enabled = variable_is_on(&value);
                    let mut state = self.state.borrow_mut();
                    state.allow_mpp = enabled;
                    state.mpp_disabled_explicitly = !enabled;
                    drop(state);
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| session_error("set planner MPP allowance", error))?;
                }
                "tidb_enforce_mpp" => {
                    let enabled = variable_is_on(&value);
                    let mut state = self.state.borrow_mut();
                    state.enforce_mpp = enabled;
                    if enabled {
                        state.allow_mpp = true;
                        state.mpp_disabled_explicitly = false;
                    }
                    drop(state);
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| session_error("set planner MPP enforcement", error))?;
                }
                "tidb_opt_tiflash_concurrency_factor" => {
                    self.state.borrow_mut().tiflash_concurrency_factor = value
                        .trim_matches(['\'', '"'])
                        .parse::<f64>()
                        .map_err(|error| {
                            session_error("parse tidb_opt_tiflash_concurrency_factor", error)
                        })?;
                }
                astersql_sessionctx_vardef::TiFlashComputeDispatchPolicy => {
                    let raw = value.trim_matches(['\'', '"']);
                    let policy =
                        astersql_sessionctx_variable::tiflashcompute::GetDispatchPolicyByStr(raw)
                            .map_err(|error| SessionError::new(error.to_string()))?;
                    if is_global {
                        self.domain
                            .set_global_tiflash_compute_dispatch_policy(policy);
                    } else {
                        self.state.borrow_mut().tiflash_compute_dispatch_policy = policy;
                    }
                }
                astersql_sessionctx_vardef::TiDBOptCorrelationExpFactor => {
                    let factor =
                        value
                            .trim_matches(['\'', '"'])
                            .parse::<i64>()
                            .map_err(|error| {
                                session_error("parse tidb_opt_correlation_exp_factor", error)
                            })?;
                    self.state.borrow_mut().correlation_exp_factor = factor;
                }
                astersql_sessionctx_vardef::TiDBOptIndexJoinMaxScanRowsRatio => {
                    let ratio =
                        value
                            .trim_matches(['\'', '"'])
                            .parse::<f64>()
                            .map_err(|error| {
                                session_error(
                                    "parse tidb_opt_index_join_max_scan_rows_ratio",
                                    error,
                                )
                            })?;
                    if ratio < 0.0 {
                        return Err(SessionError::new(
                            "tidb_opt_index_join_max_scan_rows_ratio must be non-negative",
                        ));
                    }
                    self.state.borrow_mut().repro_hash_join_max_scan_rows_ratio = ratio;
                }
                "tidb_auto_analyze_ratio" => {
                    let ratio = value
                        .trim_matches(['\'', '"'])
                        .parse::<f64>()
                        .map_err(|error| session_error("parse tidb_auto_analyze_ratio", error))?;
                    self.domain
                        .set_auto_analyze_ratio(ratio)
                        .map_err(|error| session_error("set tidb_auto_analyze_ratio", error))?;
                }
                "tidb_enable_prepared_plan_cache" => {
                    let mut state = self.state.borrow_mut();
                    state.prepared_plan_cache = variable_is_on(&value);
                    // Go drops every cached plan when the switch flips.
                    for prepared in state.prepared_by_name.values_mut() {
                        prepared.planned = false;
                        prepared.cached_transaction_contexts.clear();
                    }
                }
                "tidb_enable_non_prepared_plan_cache" => {
                    let mut state = self.state.borrow_mut();
                    state.non_prepared_plan_cache = variable_is_on(&value);
                    state.non_prepared_plan_cache_keys.clear();
                }
                "tidb_enable_dist_task" => {
                    self.state.borrow_mut().dist_task_enabled = variable_is_on(&value);
                }
                "tidb_enable_clustered_index" => {
                    let value = value.trim_matches(['\'', '"']);
                    self.state.borrow_mut().clustered_index_def_mode =
                        astersql_sessionctx_vardef::TiDBOptEnableClustered(
                            &value.to_ascii_uppercase(),
                        );
                }
                "tidb_shard_row_id_bits" => {
                    self.state.borrow_mut().shard_row_id_bits = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse tidb_shard_row_id_bits", error))?;
                }
                "tidb_pre_split_regions" => {
                    self.state.borrow_mut().pre_split_regions = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse tidb_pre_split_regions", error))?;
                }
                "tidb_ddl_enable_fast_reorg" => {
                    self.state.borrow_mut().ddl_fast_reorg_enabled = variable_is_on(&value);
                }
                "tidb_stats_update_during_ddl" => {
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| session_error("set DDL analyze", error))?;
                    self.state.borrow_mut().ddl_analyze_enabled = variable_is_on(&value);
                }
                "tidb_scatter_region" => {
                    let scatter = value.trim_matches(['\'', '"']).to_ascii_lowercase();
                    if !matches!(
                        scatter.as_str(),
                        astersql_sessionctx_vardef::ScatterOff
                            | astersql_sessionctx_vardef::ScatterTable
                            | astersql_sessionctx_vardef::ScatterGlobal
                    ) {
                        return Err(SessionError::new(format!(
                            "Variable 'tidb_scatter_region' can't be set to the value of '{value}'"
                        )));
                    }
                    if variable.IsGlobal {
                        self.domain.set_global_scatter_region(&scatter);
                    } else {
                        self.state.borrow_mut().scatter_region = scatter;
                    }
                }
                "tidb_txn_mode" => {
                    let mode = value.trim_matches(['\'', '"']).to_ascii_lowercase();
                    if variable.IsGlobal {
                        self.domain.set_global_txn_mode(&mode);
                    } else {
                        self.state.borrow_mut().txn_mode = mode;
                    }
                }
                "tidb_enable_async_commit" => {
                    self.state.borrow_mut().enable_async_commit = variable_is_on(&value);
                }
                "tidb_guarantee_linearizability" => {
                    self.state.borrow_mut().guarantee_linearizability = variable_is_on(&value);
                }
                "tidb_enable_1pc" => {
                    self.state.borrow_mut().enable_1pc = variable_is_on(&value);
                }
                astersql_sessionctx_vardef::TiDBOptFixControl => {
                    let raw_value = value.trim_matches(['\'', '"']);
                    if !is_global {
                        self.session_vars
                            .SetHintSystemVarWithOldState(&name, raw_value)
                            .map_err(|error| session_error("set optimizer fix control", error))?;
                    }
                    self.state.borrow_mut().optimizer_fix_control = raw_value.to_owned();
                }
                "tikv_client_read_timeout" => {
                    self.state.borrow_mut().tikv_client_read_timeout_ms = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse tikv_client_read_timeout", error))?;
                }
                "tidb_replica_read" => {
                    self.state.borrow_mut().replica_read =
                        value.trim_matches(['\'', '"']).to_ascii_lowercase();
                }
                "tidb_enable_paging" => {
                    self.state.borrow_mut().enable_paging = variable_is_on(&value);
                }
                astersql_sessionctx_vardef::TiDBEnableTiKVShortCircuitExpression => {
                    let enabled = variable_is_on(&value);
                    self.session_vars
                        .SetEnableTiKVShortCircuitExpression(enabled);
                    self.session_vars
                        .StmtCtx
                        .SetEnableTiKVShortCircuitExpression(enabled);
                }
                "tidb_min_paging_size" => {
                    self.state.borrow_mut().min_paging_size = value
                        .trim_matches(['\'', '"'])
                        .parse::<usize>()
                        .map_err(|error| session_error("parse tidb_min_paging_size", error))?;
                }
                "autocommit" => {
                    if !is_global {
                        let enabled = variable_is_on(&value);
                        if enabled
                            && !self.state.borrow().autocommit
                            && self.state.borrow().transaction.is_some()
                        {
                            self.finish_transaction(true)?;
                        }
                        self.state.borrow_mut().autocommit = enabled;
                    }
                }
                "tx_isolation" | "transaction_isolation" => {
                    self.state.borrow_mut().transaction_isolation = value
                        .trim_matches(['\'', '"'])
                        .replace('_', "-")
                        .to_ascii_uppercase();
                }
                "tx_isolation_one_shot" => {
                    if self.state.borrow().transaction.is_some() {
                        return Err(SessionError::new(
                            "set transaction isolation is not allowed in a transaction",
                        ));
                    }
                    self.state.borrow_mut().transaction_isolation_one_shot = Some(
                        value
                            .trim_matches(['\'', '"'])
                            .replace('_', "-")
                            .to_ascii_uppercase(),
                    );
                }
                "innodb_lock_wait_timeout" => {
                    let timeout = value
                        .trim_matches(['\'', '"'])
                        .parse::<u64>()
                        .map_err(|error| session_error("parse innodb_lock_wait_timeout", error))?
                        .min(1_073_741_824);
                    self.state.borrow_mut().innodb_lock_wait_timeout_secs = timeout;
                }
                "max_execution_time" => {
                    if !is_global {
                        self.state.borrow_mut().max_execution_time_ms = value
                            .trim_matches(['\'', '"'])
                            .parse::<u64>()
                            .map_err(|error| session_error("parse max_execution_time", error))?;
                        self.session_vars
                            .SetHintSystemVarWithOldState(&name, &value)
                            .map_err(|error| {
                                session_error("set max_execution_time system variable", error)
                            })?;
                    }
                }
                "tidb_pessimistic_txn_fair_locking" => {
                    self.state.borrow_mut().fair_locking = variable_is_on(&value);
                }
                "tidb_enable_noop_functions" => {
                    if is_global {
                        self.state.borrow_mut().global_enable_noop_functions =
                            variable_is_on(&value);
                    } else {
                        self.state.borrow_mut().enable_noop_functions = variable_is_on(&value);
                    }
                }
                "tidb_enable_shared_lock_promotion" => {
                    let enabled = variable_is_on(&value);
                    let mut state = self.state.borrow_mut();
                    if state.enable_shared_lock_promotion != enabled {
                        for prepared in state.prepared_by_name.values_mut() {
                            prepared.planned = false;
                            prepared.cached_transaction_contexts.clear();
                        }
                        state.last_plan_from_cache = false;
                    }
                    state.enable_shared_lock_promotion = enabled;
                }
                "foreign_key_checks" => {
                    self.state.borrow_mut().foreign_key_checks = variable_is_on(&value);
                }
                "tidb_foreign_key_check_in_shared_lock" => {
                    let input = value.trim_matches(['\'', '"']);
                    if is_global {
                        let (normalized, _) = self
                            .session_vars
                            .ValidateAndSetGlobalSystemVar(
                                &name,
                                input,
                                astersql_sessionctx_vardef::ScopeGlobal,
                            )
                            .map_err(|error| {
                                session_error("set global foreign key shared lock", error)
                            })?;
                        self.domain.set_global_system_variable(&name, &normalized);
                        continue;
                    }
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, input)
                        .map_err(|error| session_error("set foreign key shared lock", error))?;
                    let normalized = self
                        .session_vars
                        .GetHintSystemVar(&name)
                        .map_err(|error| session_error("read foreign key shared lock", error))?;
                    self.state.borrow_mut().foreign_key_check_in_shared_lock =
                        astersql_sessionctx_variable::TiDBOptOn(&normalized);
                }
                "tidb_enable_foreign_key" => {
                    // The Domain catalog always retains FK metadata. This
                    // switch controls enforcement only; DEFAULT restores ON.
                    self.state.borrow_mut().foreign_key_checks = variable_is_on(&value);
                }
                "tidb_constraint_check_in_place" => {
                    self.state.borrow_mut().constraint_check_in_place = variable_is_on(&value);
                }
                "tidb_constraint_check_in_place_pessimistic" => {
                    self.state
                        .borrow_mut()
                        .constraint_check_in_place_pessimistic = variable_is_on(&value);
                }
                "tidb_low_resolution_tso" => {
                    self.state.borrow_mut().low_resolution_tso = variable_is_on(&value);
                }
                "tidb_txn_entry_size_limit" => {
                    let configured = value
                        .trim_matches(['\'', '"'])
                        .parse::<usize>()
                        .map_err(|error| session_error("parse tidb_txn_entry_size_limit", error))?;
                    let limit = if configured == 0 {
                        DEFAULT_TXN_ENTRY_SIZE_LIMIT
                    } else {
                        configured
                    };
                    if variable.IsGlobal {
                        RUNTIME_GLOBAL_TXN_ENTRY_SIZE_LIMITS
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(runtime_domain_id(&self.domain), limit);
                    } else {
                        self.state.borrow_mut().txn_entry_size_limit = limit;
                    }
                }
                astersql_sessionctx_vardef::TiDBAllowAutoRandExplicitInsert => {
                    self.state.borrow_mut().allow_auto_random_explicit_insert =
                        variable_is_on(&value);
                }
                "tidb_mem_quota_query"
                | astersql_sessionctx_vardef::CTEMaxRecursionDepth
                | "tidb_dml_batch_size"
                | "tidb_batch_insert"
                | "tidb_enable_rate_limit_action"
                | astersql_sessionctx_vardef::TiDBHashJoinVersion
                | astersql_sessionctx_vardef::TiDBEnableParallelApply
                | astersql_sessionctx_vardef::TiDBExecutorConcurrency
                | astersql_sessionctx_vardef::TiDBIndexLookupJoinConcurrency
                | astersql_sessionctx_vardef::TiDBEnableCascadesPlanner
                | astersql_sessionctx_vardef::TiDBOptEnableFuzzyBinding
                | astersql_sessionctx_vardef::TiDBOptEnableHashJoin
                | astersql_sessionctx_vardef::TiDBOptEnableSemiJoinRewrite
                | astersql_sessionctx_vardef::TiDBOptEnableAdvancedJoinReorder
                | astersql_sessionctx_vardef::TiDBOptJoinReorderThreshold
                | astersql_sessionctx_vardef::TiDBOptEnableAlternativeLogicalPlans
                | astersql_sessionctx_vardef::TiDBOptExplainNoEvaledSubQuery
                | astersql_sessionctx_vardef::TiDBDefaultStrMatchSelectivity
                | astersql_sessionctx_vardef::TiDBBCJThresholdCount
                | astersql_sessionctx_vardef::TiDBBCJThresholdSize
                | astersql_sessionctx_vardef::TiDBEnableAnalyzeSnapshot
                | astersql_sessionctx_vardef::TiDBEnablePseudoForOutdatedStats => {
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| {
                            session_error("set executor memory system variable", error)
                        })?;
                }
                astersql_sessionctx_vardef::TiDBOptAdvancedJoinHint
                | astersql_sessionctx_vardef::TiDBEnableINLJoinInnerMultiPattern => {
                    if !is_global {
                        self.session_vars
                            .SetHintSystemVarWithOldState(&name, &value)
                            .map_err(|error| session_error("set optimizer join variable", error))?;
                    }
                }
                astersql_sessionctx_vardef::SQLSelectLimit => {
                    let mut state = self.state.borrow_mut();
                    for prepared in state.prepared_by_name.values_mut() {
                        prepared.planned = false;
                        prepared.cached_transaction_contexts.clear();
                    }
                    state.last_plan_from_cache = false;
                    drop(state);
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, &value)
                        .map_err(|error| session_error("set sql_select_limit", error))?;
                }
                astersql_sessionctx_vardef::TiDBRedactLog => {
                    let mode = value.trim_matches(['\'', '"']).to_ascii_uppercase();
                    if !matches!(
                        mode.as_str(),
                        astersql_sessionctx_vardef::Off
                            | astersql_sessionctx_vardef::On
                            | astersql_sessionctx_vardef::Marker
                    ) {
                        return Err(SessionError::new(format!(
                            "Variable '{name}' can't be set to the value of '{value}'"
                        )));
                    }
                    self.state.borrow_mut().redact_log = mode;
                }
                astersql_sessionctx_vardef::TiDBDDLReorgWorkerCount
                | astersql_sessionctx_vardef::TiDBDDLReorgBatchSize
                | astersql_sessionctx_vardef::TiDBMaxDistTaskNodes => {
                    if is_global {
                        let (normalized, warnings) = self
                            .session_vars
                            .ValidateAndSetGlobalSystemVar(
                                &name,
                                value.trim_matches(['\'', '"']),
                                astersql_sessionctx_vardef::ScopeGlobal,
                            )
                            .map_err(|error| {
                                session_error("set global DDL reorg variable", error)
                            })?;
                        for warning in warnings {
                            self.set_warning(warning.to_string());
                        }
                        self.domain
                            .set_stats_global_variable(&name, &normalized)
                            .map_err(|error| {
                                session_error("persist global DDL reorg variable", error)
                            })?;
                        continue;
                    }
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, value.trim_matches(['\'', '"']))
                        .map_err(|error| session_error("set DDL reorg system variable", error))?;
                }
                astersql_sessionctx_vardef::TiDBRetryLimit
                | astersql_sessionctx_vardef::TiDBDistSQLScanConcurrency => {
                    if is_global {
                        let (normalized, warnings) = self
                            .session_vars
                            .ValidateAndSetGlobalSystemVar(
                                &name,
                                value.trim_matches(['\'', '"']),
                                astersql_sessionctx_vardef::ScopeGlobal,
                            )
                            .map_err(|error| session_error("set TTL session variable", error))?;
                        for warning in warnings {
                            self.set_warning(warning.to_string());
                        }
                        self.domain.set_global_system_variable(&name, &normalized);
                        continue;
                    }
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, value.trim_matches(['\'', '"']))
                        .map_err(|error| session_error("set TTL session variable", error))?;
                }
                astersql_sessionctx_vardef::TiDBMLogPurgeBatchSize
                | astersql_sessionctx_vardef::TiDBMLogPurgeMinRate
                | astersql_sessionctx_vardef::TiDBMLogPurgeRateBudgetRatio
                | astersql_sessionctx_vardef::TiDBMLogPurgeDeleteTiFlashThreads => {
                    self.session_vars
                        .SetHintSystemVarWithOldState(&name, value.trim_matches(['\'', '"']))
                        .map_err(|error| session_error("set MLog purge system variable", error))?;
                }
                _ => {}
            }
            if is_global {
                let raw_value = value.trim_matches(['\'', '"']);
                if name == astersql_sessionctx_vardef::TiDBTTLJobEnable {
                    let enabled = astersql_sessionctx_variable::TiDBOptOn(raw_value);
                    self.domain
                        .update_external_workload_ttl_job_enable(
                            &astersql_extworkload::context::Background(),
                            enabled,
                        )
                        .map_err(SessionError::new)?;
                    astersql_sessionctx_vardef::EnableTTLJob.Store(enabled);
                }
                if name == astersql_sessionctx_vardef::TiDBTTLEnableIndexScan {
                    astersql_sessionctx_vardef::TTLEnableIndexScan
                        .Store(astersql_sessionctx_variable::TiDBOptOn(raw_value));
                }
                let global_value = astersql_sessionctx_variable::GetSysVar(&name).map_or_else(
                    || raw_value.to_owned(),
                    |variable| {
                        if variable.Type == astersql_sessionctx_vardef::TypeBool {
                            astersql_sessionctx_variable::BoolToOnOff(
                                astersql_sessionctx_variable::TiDBOptOn(raw_value),
                            )
                        } else {
                            raw_value.to_owned()
                        }
                    },
                );
                self.domain.set_global_system_variable(&name, &global_value);
            }
        }
        Ok(())
    }
}

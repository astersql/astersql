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

//! Read-only, lazy target-keyspace global variable cache.
use astersql_ddl_schemaver as etcd;
use astersql_sessionctx_variable::{self as variable, GlobalVarAccessor, VariableError};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub use astersql_ddl_util::SysVarCacheKey as SYS_VAR_CACHE_KEY;
pub type Query = dyn Fn(&str) -> Result<Vec<Vec<String>>, String> + Send + Sync;

/// Clones share immutable snapshots and one serialized load/refresh lifecycle.
#[derive(Clone)]
pub struct GlobalVarCache(Arc<Inner>);
struct Inner {
    keyspace: String,
    query: Arc<Query>,
    etcd: Option<Arc<dyn etcd::EtcdClient>>,
    context: etcd::Context,
    values: RwLock<Option<Arc<HashMap<String, String>>>>,
    load: Mutex<()>,
    started: AtomicBool,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl GlobalVarCache {
    pub fn new(
        keyspace: String,
        query: Arc<Query>,
        etcd: Option<Arc<dyn etcd::EtcdClient>>,
    ) -> Self {
        Self(Arc::new(Inner {
            keyspace,
            query,
            etcd,
            context: etcd::Context::Background(),
            values: RwLock::new(None),
            load: Mutex::new(()),
            started: AtomicBool::new(false),
            worker: Mutex::new(None),
        }))
    }
    fn error(&self, name: &str) -> VariableError {
        VariableError::new(
            variable::VariableErrorKind::GlobalVariable,
            format!(
                "cannot set {name} from a session of the runtime of keyspace {}",
                self.0.keyspace
            ),
        )
    }
    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, VariableError> {
        if self.0.context.Done() {
            return Err(VariableError::new(
                variable::VariableErrorKind::GlobalVariable,
                "target runtime is closed",
            ));
        }
        (self.0.query)(sql)
            .map_err(|e| VariableError::new(variable::VariableErrorKind::GlobalVariable, e))
    }
    fn load_locked(&self) -> Result<Arc<HashMap<String, String>>, VariableError> {
        let rows =
            self.query("SELECT variable_name, variable_value FROM mysql.global_variables")?;
        let mut table = HashMap::new();
        for row in rows {
            if row.len() != 2 {
                return Err(VariableError::new(
                    variable::VariableErrorKind::GlobalVariable,
                    "unexpected global variable row",
                ));
            }
            table.insert(row[0].clone(), row[1].clone());
        }
        let (_, mut global) = variable::sysvar::SysVarCacheValues(&mut table);
        let mut vars = variable::SessionVars::new(Box::new(self.clone()));
        for (name, value) in &mut global {
            if let Some(sv) = variable::GetSysVar(name) {
                *value = sv
                    .ValidateFromType(&mut vars, value, variable::vardef::ScopeGlobal)
                    .unwrap_or_else(|_| sv.Value.clone());
            }
        }
        let global = Arc::new(global);
        *self.0.values.write().unwrap() = Some(global.clone());
        Ok(global)
    }
    /// Failed refreshes preserve the previous complete snapshot.
    pub fn reload(&self) -> Result<(), VariableError> {
        let _load = self.0.load.lock().unwrap();
        self.load_locked().map(|_| ())
    }
    pub fn refresh_started(&self) -> bool {
        self.0.started.load(Ordering::Acquire)
    }
    fn start(&self) {
        let mut worker = self.0.worker.lock().unwrap();
        if self.0.started.swap(true, Ordering::AcqRel) || self.0.context.Done() {
            return;
        }
        let cache = self.clone();
        *worker = Some(std::thread::spawn(move || cache.refresh_loop()));
    }
    fn refresh_loop(&self) {
        let mut watch = self
            .0
            .etcd
            .as_ref()
            .map(|client| client.Watch(&self.0.context, SYS_VAR_CACHE_KEY, false, 0));
        let mut next = Instant::now() + Duration::from_secs(30);
        let mut closed = 0u64;
        while !self.0.context.Done() {
            let changed = if let Some(channel) = &watch {
                match channel.RecvTimeout(Duration::from_millis(50)) {
                    Ok(_) => true,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        eprintln!(
                            "global variable cache watch channel closed for {}",
                            self.0.keyspace
                        );
                        watch = self.0.etcd.as_ref().map(|client| {
                            client.Watch(&self.0.context, SYS_VAR_CACHE_KEY, false, 0)
                        });
                        closed += 1;
                        if closed > 10 {
                            self.0.context.Wait(Duration::from_secs(closed));
                        }
                        next = Instant::now() + Duration::from_secs(30);
                        continue;
                    }
                }
            } else {
                self.0.context.Wait(Duration::from_millis(50));
                false
            };
            if changed || Instant::now() >= next {
                closed = 0;
                if let Err(error) = self.reload() {
                    eprintln!(
                        "reload global variable cache for {}: {error}",
                        self.0.keyspace
                    );
                }
                next = Instant::now() + Duration::from_secs(30);
            }
        }
    }
    /// Cancel and join before the target pool or Store is closed.
    pub fn close(&self) {
        self.0.context.Cancel();
        if let Some(worker) = self.0.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}
impl GlobalVarAccessor for GlobalVarCache {
    fn get_global_sys_var_from_hook(&self, name: &str) -> Result<String, VariableError> {
        let sv = variable::GetSysVar(name).ok_or_else(|| VariableError::unknown(name))?;
        if sv.GetGlobal.is_some() {
            // Global hooks in the registry read process state or this accessor;
            // use the canonical hook dispatcher rather than the cached row.
            let mut vars = variable::SessionVars::new(Box::new(self.clone()));
            sv.GetGlobalFromHook(&variable::Context::default(), &mut vars)
        } else {
            self.get_global_sys_var(name)
        }
    }
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        let sv = variable::GetSysVar(name).ok_or_else(|| VariableError::unknown(name))?;
        let mut values = self.0.values.read().unwrap().clone();
        if values.is_none() {
            let _load = self.0.load.lock().unwrap();
            values = self.0.values.read().unwrap().clone();
            if values.is_none() {
                values = Some(self.load_locked()?);
            }
            self.start();
        }
        Ok(values
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_else(|| sv.Value.clone()))
    }
    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        let rows = self.query(&format!(
            "SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME='{}'",
            name.replace('\\', "\\\\").replace('\'', "''")
        ))?;
        rows.first()
            .and_then(|row| row.first())
            .cloned()
            .ok_or_else(|| {
                VariableError::new(
                    variable::VariableErrorKind::GlobalVariable,
                    format!("{name} not found in mysql.tidb"),
                )
            })
    }
    fn set_global_sys_var_only(
        &mut self,
        _: &variable::Context,
        name: &str,
        _: &str,
        _: bool,
    ) -> Result<(), VariableError> {
        Err(self.error(name))
    }
    fn set_tidb_table_value(&mut self, name: &str, _: &str, _: &str) -> Result<(), VariableError> {
        Err(self.error(name))
    }
}

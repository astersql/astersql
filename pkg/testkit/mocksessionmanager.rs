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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    pub id: u64,
    pub user: String,
    pub database: String,
    pub command: String,
    pub started_at: SystemTime,
    /// Kept for compatibility with early Rust callers. Go's `Kill` never
    /// changes process state.
    pub killed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TxnInfo {
    pub connection_id: u64,
    pub start_ts: u64,
    pub current_sql_digest: Option<String>,
    /// Go only returns a connection-derived transaction when ProcessInfo exists.
    pub has_process_info: bool,
}

/// The subset of `sessionapi.Session` observed by the Go mock.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionSnapshot {
    pub process_info: Option<ProcessInfo>,
    pub txn_info: Option<TxnInfo>,
    pub connection_id: u64,
    pub flashback_cluster_statement: bool,
    pub lock_ddl_jobs: HashSet<i64>,
}

#[derive(Default)]
struct State {
    explicit_transactions: Vec<TxnInfo>,
    connections: HashMap<u64, SessionSnapshot>,
    connection_attributes: HashMap<u64, HashMap<String, String>>,
    internal_sessions: HashMap<u64, Option<TxnInfo>>,
    server_id: u64,
}

/// Thread-safe counterpart of Go's `testkit.MockSessionManager`.
#[derive(Clone, Default)]
pub struct MockSessionManager {
    /// Go calls this `PS` and protects it independently with `PSMu`.
    processes: Arc<RwLock<Vec<ProcessInfo>>>,
    state: Arc<Mutex<State>>,
}

impl MockSessionManager {
    /// Adds or replaces an explicit `PS` entry.
    pub fn StoreProcessInfo(&self, info: ProcessInfo) {
        let mut processes = self.processes.write().expect("process list poisoned");
        if let Some(existing) = processes.iter_mut().find(|item| item.id == info.id) {
            *existing = info;
        } else {
            processes.push(info);
        }
    }

    pub fn DeleteProcessInfo(&self, connection_id: u64) {
        self.processes
            .write()
            .expect("process list poisoned")
            .retain(|item| item.id != connection_id);
        self.state
            .lock()
            .expect("session manager poisoned")
            .explicit_transactions
            .retain(|item| item.connection_id != connection_id);
    }

    /// Registers a live connection used when explicit fixtures are empty.
    pub fn StoreConnection(&self, id: u64, session: SessionSnapshot) {
        self.state
            .lock()
            .expect("session manager poisoned")
            .connections
            .insert(id, session);
    }

    pub fn DeleteConnection(&self, id: u64) {
        self.state
            .lock()
            .expect("session manager poisoned")
            .connections
            .remove(&id);
    }

    pub fn ShowProcessList(&self) -> HashMap<u64, ProcessInfo> {
        let processes = self.processes.read().expect("process list poisoned");
        if !processes.is_empty() {
            return processes
                .iter()
                .cloned()
                .map(|item| (item.id, item))
                .collect();
        }
        drop(processes);
        self.state
            .lock()
            .expect("session manager poisoned")
            .connections
            .iter()
            .filter_map(|(id, session)| session.process_info.clone().map(|info| (*id, info)))
            .collect()
    }

    pub fn GetProcessInfo(&self, id: u64) -> Option<ProcessInfo> {
        if let Some(info) = self
            .processes
            .read()
            .expect("process list poisoned")
            .iter()
            .find(|item| item.id == id)
            .cloned()
        {
            return Some(info);
        }
        self.state
            .lock()
            .expect("session manager poisoned")
            .connections
            .get(&id)
            .and_then(|session| session.process_info.clone())
    }

    /// Go's test double intentionally ignores every kill request.
    pub fn Kill(&self, _connection_id: u64) -> bool {
        false
    }

    pub fn KillAllConnections(&self) {}

    pub fn UpdateTLSConfig(&self) {}

    pub fn SetServerID(&self, server_id: u64) {
        self.state
            .lock()
            .expect("session manager poisoned")
            .server_id = server_id;
    }

    pub fn ServerID(&self) -> u64 {
        self.state
            .lock()
            .expect("session manager poisoned")
            .server_id
    }

    pub fn SetTxnInfo(&self, transaction: TxnInfo) {
        let mut state = self.state.lock().expect("session manager poisoned");
        if let Some(existing) = state
            .explicit_transactions
            .iter_mut()
            .find(|item| item.connection_id == transaction.connection_id)
        {
            *existing = transaction;
        } else {
            state.explicit_transactions.push(transaction);
        }
    }

    pub fn GetTxnInfo(&self, connection_id: u64) -> Option<TxnInfo> {
        let state = self.state.lock().expect("session manager poisoned");
        state
            .explicit_transactions
            .iter()
            .find(|item| item.connection_id == connection_id)
            .cloned()
            .or_else(|| {
                state
                    .connections
                    .get(&connection_id)
                    .and_then(|session| session.txn_info.clone())
            })
    }

    pub fn ShowTxnList(&self) -> Vec<TxnInfo> {
        let state = self.state.lock().expect("session manager poisoned");
        if !state.explicit_transactions.is_empty() {
            return state.explicit_transactions.clone();
        }
        state
            .connections
            .values()
            .filter_map(|session| session.txn_info.clone())
            .filter(|txn| txn.has_process_info)
            .collect()
    }

    pub fn SetConAttrs(&self, attrs: HashMap<u64, HashMap<String, String>>) {
        self.state
            .lock()
            .expect("session manager poisoned")
            .connection_attributes = attrs;
    }

    pub fn GetConAttrs(&self) -> HashMap<u64, HashMap<String, String>> {
        self.state
            .lock()
            .expect("session manager poisoned")
            .connection_attributes
            .clone()
    }

    pub fn StoreInternalSession(&self, id: u64, txn_info: Option<TxnInfo>) {
        self.state
            .lock()
            .expect("session manager poisoned")
            .internal_sessions
            .insert(id, txn_info);
    }

    pub fn ContainsInternalSession(&self, id: u64) -> bool {
        self.state
            .lock()
            .expect("session manager poisoned")
            .internal_sessions
            .contains_key(&id)
    }

    pub fn InternalSessionCount(&self) -> usize {
        self.state
            .lock()
            .expect("session manager poisoned")
            .internal_sessions
            .len()
    }

    pub fn DeleteInternalSession(&self, id: u64) {
        self.state
            .lock()
            .expect("session manager poisoned")
            .internal_sessions
            .remove(&id);
    }

    /// Go returns map iteration order; callers must not rely on sorting.
    pub fn GetInternalSessionStartTSList(&self) -> Vec<u64> {
        self.state
            .lock()
            .expect("session manager poisoned")
            .internal_sessions
            .values()
            .filter_map(|txn| txn.as_ref().map(|txn| txn.start_ts))
            .collect()
    }

    /// `Kill` is a no-op in the Go mock, so traversal has no mutation.
    pub fn KillNonFlashbackClusterConn(&self) {
        let state = self.state.lock().expect("session manager poisoned");
        for session in state.connections.values() {
            if !session.flashback_cluster_statement {
                let _ = self.Kill(session.connection_id);
            }
        }
    }

    pub fn CheckOldRunningTxn(&self, jobs: &mut HashSet<i64>) {
        let state = self.state.lock().expect("session manager poisoned");
        for session in state.connections.values() {
            jobs.retain(|job| !session.lock_ddl_jobs.contains(job));
        }
    }

    pub fn GetStatusVars(&self) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
}

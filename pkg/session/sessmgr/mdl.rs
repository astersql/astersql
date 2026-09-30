// Copyright 2026 AsterSQL.

use crate::mdldef::JobMDL;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Per-table versions held by a real transaction, readable by schema loops
/// without borrowing the thread-local SQL session. A zero version fences the
/// interval between first table access and loading its latest metadata.
#[derive(Default)]
pub struct TransactionMDL {
    versions: Mutex<HashMap<i64, i64>>,
    restricted: AtomicBool,
}
impl TransactionMDL {
    pub fn set_restricted(&self, restricted: bool) {
        self.restricted.store(restricted, Ordering::Release);
    }
    pub fn begin_table(&self, id: i64) {
        self.versions.lock().unwrap().entry(id).or_insert(0);
    }
    pub fn finish_table(&self, id: i64, version: i64) {
        self.versions.lock().unwrap().insert(id, version);
    }
    pub fn remove_table(&self, id: i64) {
        self.versions.lock().unwrap().remove(&id);
    }
    pub fn clear(&self) {
        self.versions.lock().unwrap().clear();
    }
    /// Like RemoveLockDDLJobs, remove blocked jobs, never release a txn lock.
    pub fn check_jobs(&self, jobs: &mut HashMap<i64, Arc<JobMDL>>) {
        if self.restricted.load(Ordering::Acquire) {
            return;
        }
        let versions = self.versions.lock().unwrap();
        jobs.retain(|_, job| {
            !job.table_ids
                .iter()
                .any(|id| versions.get(id).is_some_and(|version| *version < job.ver))
        });
    }
}

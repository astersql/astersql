// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use astersql_kv::Key;
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// Whole data row keys preserve physical table identity for partitioned rows.
pub struct BoundedKeySet {
    shared_size: Arc<AtomicI64>,
    size_limit: i64,
    keys: HashSet<Vec<u8>>,
}
/// Create a bounded set, reserving no entries when the shared budget is exhausted.
pub fn NewBoundedKeySet(shared_size: Arc<AtomicI64>, limit: i64) -> BoundedKeySet {
    let capacity = if shared_size.load(Ordering::Acquire) >= limit {
        0
    } else {
        128
    };
    BoundedKeySet {
        shared_size,
        size_limit: limit,
        keys: HashSet::with_capacity(capacity),
    }
}
impl BoundedKeySet {
    /// Record a complete row key and charge its bytes plus Go map entry overhead.
    pub fn Add(&mut self, key: &Key) {
        if self.BoundExceeded() {
            return;
        }
        // Match Go string header + bool, including repeated Add accounting.
        let delta = key.0.len() as i64 + std::mem::size_of::<&str>() as i64 + 1;
        self.shared_size.fetch_add(delta, Ordering::AcqRel);
        self.keys.insert(key.0.clone());
    }
    /// Check row identity without conflating partitions or common-handle strings.
    pub fn Contains(&self, key: &Key) -> bool {
        self.keys.contains(&key.0)
    }
    /// Merge retained keys without charging the shared budget a second time.
    pub fn Merge(&mut self, other: Option<&Self>) {
        if let Some(other) = other {
            self.keys.extend(other.keys.iter().cloned());
        }
    }
    /// Return whether the shared byte budget has reached its limit.
    pub fn BoundExceeded(&self) -> bool {
        self.shared_size.load(Ordering::Acquire) >= self.size_limit
    }
    /// Number of distinct retained row keys.
    pub fn Len(&self) -> usize {
        self.keys.len()
    }
    /// Current shared memory accounting across worker sets.
    pub fn SharedSize(&self) -> i64 {
        self.shared_size.load(Ordering::Acquire)
    }
}
/// Global sets span index groups; each worker retains its local set across batches.
pub struct KeyFilter {
    global: Arc<BoundedKeySet>,
    local: Arc<Mutex<BoundedKeySet>>,
}
/// Construct separate filters for earlier groups and this worker's successful rows.
pub fn NewKeyFilter(global: Arc<BoundedKeySet>, local: Arc<Mutex<BoundedKeySet>>) -> KeyFilter {
    KeyFilter { global, local }
}
impl KeyFilter {
    /// Check whether an earlier index group already processed this row.
    pub fn isHandledGlobally(&self, key: &Key) -> bool {
        self.global.Contains(key)
    }
    /// Check whether this worker already processed the row in an earlier batch.
    pub fn isHandledLocally(&self, key: &Key) -> bool {
        self.local.lock().unwrap().Contains(key)
    }
    /// Register a row only after its callback succeeds.
    pub fn addLocal(&self, key: &Key) {
        self.local.lock().unwrap().Add(key);
    }
}

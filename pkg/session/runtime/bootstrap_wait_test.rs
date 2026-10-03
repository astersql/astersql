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

use super::bootstrap_wait::{
    BootstrapWaitClock, check_system_bootstrap_version, must_get_store_bootstrap_version,
    wait_system_boot_version_with_clock,
};
use astersql_kv as kv;
use astersql_kv::Storage;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage};
use std::sync::Arc;
use std::time::Duration;

fn store() -> impl kv::Storage {
    Arc::try_unwrap(NewMockStorage(KVStore::NewMemoryWithWallClockTSO(), None).unwrap())
        .ok()
        .unwrap()
}
fn finish(store: &dyn kv::Storage, version: i64) {
    super::bootstrap_wait::finish_store_bootstrap_version(store, version);
}
struct Clock<'a> {
    sleeps: Vec<Duration>,
    logs: Vec<Duration>,
    publish: Option<&'a dyn kv::Storage>,
}
impl BootstrapWaitClock for Clock<'_> {
    fn sleep(&mut self, duration: Duration) {
        self.sleeps.push(duration);
        if self.logs.len() == 1 {
            if let Some(store) = self.publish.take() {
                finish(store, 300);
            }
        }
    }
    fn log_wait(&mut self) {
        self.logs.push(self.sleeps.iter().sum());
    }
}
fn clock() -> Clock<'static> {
    Clock {
        sleeps: vec![],
        logs: vec![],
        publish: None,
    }
}
#[test]
fn bootstrap_wait_reads_committed_version_after_retryable_commit() {
    let store = store();
    finish(&store, 300);
    let scenario = fail::FailScenario::setup();
    fail::cfg("mockCommitErrorInNewTxn", "return(retry_once)").unwrap();
    assert_eq!(must_get_store_bootstrap_version(&store), 300);
    drop(scenario);
}
#[test]
fn bootstrap_wait_observes_system_commit_after_logged_retry() {
    let store = store();
    let mut clock = Clock {
        publish: Some(&store),
        ..clock()
    };
    assert_eq!(wait_system_boot_version_with_clock(&store, &mut clock), 300);
    assert_eq!(clock.sleeps, [1, 2, 4, 5, 5].map(Duration::from_secs));
    assert_eq!(clock.logs, vec![Duration::from_secs(12)]);
}
#[test]
fn bootstrap_wait_exhausts_budget_after_reset() {
    let store = store();
    finish(&store, 300);
    finish(&store, 0);
    let mut clock = clock();
    assert_eq!(wait_system_boot_version_with_clock(&store, &mut clock), 0);
    assert_eq!(clock.sleeps.len(), 360);
    assert_eq!(
        clock.sleeps.iter().sum::<Duration>(),
        Duration::from_secs(1792)
    );
    assert_eq!(clock.logs.len(), 72);
    assert_eq!(clock.sleeps[359], Duration::from_secs(5));
}
#[test]
fn bootstrap_wait_returns_ready_version_without_sleep_and_preserves_guards() {
    let store = store();
    finish(&store, 300);
    let mut clock = clock();
    assert_eq!(wait_system_boot_version_with_clock(&store, &mut clock), 300);
    assert!(clock.sleeps.is_empty());
    assert!(clock.logs.is_empty());
    check_system_bootstrap_version(&store, 300, 299, &mut clock);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_system_bootstrap_version(&store, 301, 299, &mut clock)
        }))
        .is_err()
    );
    finish(&store, 0);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_system_bootstrap_version(&store, 300, 299, &mut clock)
        }))
        .is_err()
    );
}
#[test]
fn bootstrap_wait_rejects_invalid_metadata_and_nonretryable_commit() {
    let store = store();
    let mut txn = store.Begin(&[]).unwrap();
    txn.Set(
        astersql_meta::transaction_meta_string_key(b"BootstrapKey"),
        b"invalid".to_vec(),
    )
    .unwrap();
    txn.Commit(&kv::Context::default()).unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            must_get_store_bootstrap_version(&store)
        }))
        .is_err()
    );
    finish(&store, 300);
    let scenario = fail::FailScenario::setup();
    fail::cfg("mockCommitErrorInNewTxn", "return(no_retry)").unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            must_get_store_bootstrap_version(&store)
        }))
        .is_err()
    );
    drop(scenario);
}

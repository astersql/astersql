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

use crate::orphandata::*;
use anyhow::Result;
use astersql_objstore::memstore::NewMemStorage;
use astersql_objstore::storage::{Context, Storage};
use prometheus::core::Collector;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

static METRIC_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[test]
fn orphan_scan_counts_bytes_samples_and_retains_conflict_rows() {
    let context = Context::default();
    let store = NewMemStorage();
    store.WriteFile(&context, "123/meta.json", b"123").unwrap();
    store
        .WriteFile(&context, "conflicted-rows/9/3-uuid/0_1", b"conflict")
        .unwrap();
    for index in 0..=SAMPLE_OBJECT_LIMIT {
        store
            .WriteFile(&context, &format!("unknown/{index:02}"), b"x")
            .unwrap();
    }

    let stats = scan_orphan_data(&context, &store, &["conflicted-rows/".into()]).unwrap();
    assert_eq!(stats.size_bytes, 3 + (SAMPLE_OBJECT_LIMIT + 1) as i64);
    assert_eq!(stats.object_count, (SAMPLE_OBJECT_LIMIT + 2) as i64);
    assert_eq!(stats.sample_objects.len(), SAMPLE_OBJECT_LIMIT);
    assert!(stats.truncated);
}

#[test]
fn orphan_monitor_uses_two_active_producer_gates() {
    let _guard = METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe { astersql_metrics::globalsort::InitGlobalSortMetrics() };
    let checks = Arc::new(AtomicUsize::new(0));
    let checker = checks.clone();
    let monitor = Monitor::new(Config {
        active_producer_checker: Arc::new(move || -> Result<bool> {
            Ok(checker.fetch_add(1, Ordering::SeqCst) == 1)
        }),
        get_storage_uri: Arc::new(|| "memstore://orphan".into()),
        retained_prefixes: vec![],
        store_factory: None,
    });
    monitor.trigger(&Context::default());
    assert_eq!(checks.load(Ordering::SeqCst), 2);
}

#[test]
fn orphan_monitor_keeps_old_value_when_a_producer_appears_after_scan() {
    let _guard = METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let gauge = &*astersql_metrics::globalsort::GlobalSortOrphanDataSize;
    gauge.with_label_values(&[] as &[&str]).set(37.0);
    let checks = Arc::new(AtomicUsize::new(0));
    let checker = checks.clone();
    let store = Arc::new(NewMemStorage());
    store
        .WriteFile(&Context::default(), "candidate/file", &[0; 41])
        .unwrap();
    let monitor = Monitor::new(Config {
        active_producer_checker: Arc::new(move || -> Result<bool> {
            Ok(checker.fetch_add(1, Ordering::SeqCst) == 1)
        }),
        get_storage_uri: Arc::new(|| "memstore://orphan".into()),
        retained_prefixes: vec![],
        store_factory: Some(Arc::new(move |_, _| Ok(store.clone()))),
    });

    monitor.trigger(&Context::default());

    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert_eq!(gauge.with_label_values(&[] as &[&str]).get(), 37.0);
    gauge.reset();
}

#[test]
fn orphan_monitor_first_gate_skips_storage_and_reset_removes_series() {
    let _guard = METRIC_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let gauge = &*astersql_metrics::globalsort::GlobalSortOrphanDataSize;
    gauge.with_label_values(&[] as &[&str]).set(37.0);
    let factory_calls = Arc::new(AtomicUsize::new(0));
    let calls = factory_calls.clone();
    let monitor = Monitor::new(Config {
        active_producer_checker: Arc::new(|| -> Result<bool> { Ok(true) }),
        get_storage_uri: Arc::new(|| "memstore://orphan".into()),
        retained_prefixes: vec![],
        store_factory: Some(Arc::new(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(NewMemStorage()))
        })),
    });

    monitor.trigger(&Context::default());
    assert_eq!(factory_calls.load(Ordering::SeqCst), 0);
    assert_eq!(gauge.with_label_values(&[] as &[&str]).get(), 37.0);

    monitor.reset();
    assert!(
        gauge
            .collect()
            .iter()
            .all(|family| family.get_metric().is_empty())
    );
}

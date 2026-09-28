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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::disk_root::{DiskRoot, ResourceTracker, risk_of_disk_full};

struct Tracker(AtomicU64);

impl ResourceTracker for Tracker {
    fn disk_usage(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[test]
fn update_usage_caches_backend_usage_like_go() {
    let root = DiskRoot::new("/sort", 1_000, 800);
    let tracker = Arc::new(Tracker(AtomicU64::new(80)));
    root.add(7, tracker.clone());

    root.update_usage(1_000, 800);
    assert_eq!(root.tracked_usage(), 80);
    assert_eq!(root.usage_info(), "disk usage: 200/1000, backend usage: 80");

    tracker.0.store(900, Ordering::Relaxed);
    assert_eq!(
        root.tracked_usage(),
        80,
        "Go exposes the last UpdateUsage snapshot"
    );
    root.update_usage(1_000, 700);
    assert_eq!(root.tracked_usage(), 900);
}

#[test]
fn import_decision_uses_quota_and_used_capacity_not_available_bytes() {
    let root = DiskRoot::new_with_quota("/sort", 1_000, 800, 100);
    let tracker = Arc::new(Tracker(AtomicU64::new(101)));
    root.add(1, tracker);
    root.update_usage(1_000, 800);
    assert!(
        root.should_import(),
        "backend usage above DDL quota must import"
    );

    let root = DiskRoot::new_with_quota("/sort", 1_000, 100, u64::MAX);
    root.update_usage(1_000, 100);
    assert!(root.should_import(), "exactly 90% disk usage must import");
}

#[test]
fn risk_threshold_matches_go_floating_point_boundary() {
    assert!(risk_of_disk_full(0, 9));
    assert!(!risk_of_disk_full(10, 100));
    assert!(risk_of_disk_full(9, 100));
}

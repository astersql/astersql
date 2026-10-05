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

use crate::disk_root::{
    DiskRoot, LOCAL_SORT_HEADROOM_BYTES_PER_SLOT, LocalSortDiskSpaceCheck, ResourceTracker,
    check_local_sort_disk_space, min_free_disk_bytes, risk_of_disk_full,
};

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
    assert_eq!(min_free_disk_bytes(100), 10);
    assert_eq!(min_free_disk_bytes(101), 11);
    assert!(risk_of_disk_full(0, 9));
    assert!(!risk_of_disk_full(10, 100));
    assert!(risk_of_disk_full(9, 100));
    assert!(!risk_of_disk_full(11, 101));
    assert!(risk_of_disk_full(10, 101));
}

#[test]
fn local_sort_disk_space_matches_go_threshold_quota_and_message() {
    const GIB: u64 = 1024 * 1024 * 1024;
    const EXEC_ID: &str = "10.0.1.8:4000";

    let check = |available_bytes, total_capacity_bytes, runtime_slots, quota| {
        check_local_sort_disk_space(LocalSortDiskSpaceCheck {
            exec_id: EXEC_ID,
            sort_path: "/tmp/local-sort",
            available_bytes,
            total_capacity_bytes,
            current_task_runtime_slots: runtime_slots,
            ddl_disk_quota: quota,
        })
    };

    assert!(check(7 * GIB, 20 * GIB, 2, 100 * GIB).is_ok());
    let equal = check(6 * GIB, 20 * GIB, 2, 100 * GIB).unwrap_err();
    assert!(equal.is_ingest_check_env_failed());
    assert!(equal.to_string().contains(
        "6442450944 bytes available; available free disk space must be greater than 6442450944 bytes"
    ));
    assert!(!equal.to_string().contains("bytes required"));

    assert!(check(2 * GIB, 10 * GIB, 1, 100 * GIB).is_err());
    assert!(check(121 * GIB, 200 * GIB, 60, 100 * GIB).is_ok());
    assert!(check(120 * GIB, 200 * GIB, 60, 100 * GIB).is_err());

    let user_error = check(GIB, GIB, 1, 100 * GIB).unwrap_err().to_string();
    assert!(user_error.contains(
        "insufficient free disk space on TiDB node 10.0.1.8:4000 at /tmp/local-sort: 1073741824 bytes available; available free disk space must be greater than 2254857831 bytes"
    ));
    assert!(user_error.contains(
        "the add-index job cannot start because low disk space would degrade SST ingestion"
    ));
    assert!(
        user_error
            .contains("Free disk space on this TiDB node by removing unnecessary logs or files")
    );
    assert!(!user_error.contains("runtime slots"));
    assert!(!user_error.contains("bytes per slot"));
    assert_eq!(LOCAL_SORT_HEADROOM_BYTES_PER_SLOT, 2 * GIB);
}

#[test]
fn precheck_uses_real_filesystem_and_preserves_error_class() {
    let path = std::env::temp_dir().join(format!("aster-d0dfde35b7-disk-{}", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.0.is_file() {
                let _ = std::fs::remove_file(&self.0);
            } else {
                let _ = std::fs::remove_dir(&self.0);
            }
        }
    }
    let _cleanup = Cleanup(path.clone());
    std::fs::write(&path, b"not a directory").unwrap();
    let root = DiskRoot::new(path.to_string_lossy(), u64::MAX, u64::MAX);
    let error = root.pre_check_usage().unwrap_err();
    assert!(error.contains("[ddl:"), "{error}");
    std::fs::remove_file(&path).unwrap();
    root.pre_check_usage().unwrap();
    assert!(path.is_dir());
    std::fs::remove_dir(&path).unwrap();
}

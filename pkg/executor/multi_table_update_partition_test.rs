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

use crate::update::{PartitionRowLocation, UpdateRowKey, UpdateRowTracker};

#[test]
fn multi_table_update_partition_tracks_physical_row_identity() {
    let mut tracker = UpdateRowTracker::<i64, Vec<i64>>::default();

    // EXCHANGE PARTITION can leave the same _tidb_rowid in two partitions.
    let first = tracker.prepare(0, Some(&1), true, PartitionRowLocation::NonClustered(101));
    assert!(first.matches);
    assert!(!first.changed);
    assert!(tracker.record_updated(0, first.row_key.as_ref(), true));
    assert!(tracker.set_merged_row(7, first.row_key.clone().unwrap(), vec![101, 10]));

    let second = tracker.prepare(0, Some(&1), true, PartitionRowLocation::NonClustered(102));
    assert!(
        second.matches,
        "a different partition must remain updateable"
    );
    assert!(!second.changed);
    assert!(tracker.record_updated(0, second.row_key.as_ref(), true));
    assert!(tracker.set_merged_row(7, second.row_key.clone().unwrap(), vec![102, 11]));
    assert_eq!(
        tracker.merged_row(7, first.row_key.as_ref().unwrap()),
        Some(&vec![101, 10])
    );
    assert_eq!(
        tracker.merged_row(7, second.row_key.as_ref().unwrap()),
        Some(&vec![102, 11])
    );

    // The same physical row matched by another joined row is updated once.
    let repeated = tracker.prepare(0, Some(&1), true, PartitionRowLocation::NonClustered(102));
    assert!(!repeated.matches);
    assert!(repeated.changed);

    // A single-table UPDATE sees each row once and intentionally avoids partition lookup/tracking.
    let single = tracker.prepare(0, Some(&1), false, PartitionRowLocation::NonClustered(101));
    assert_eq!(single.row_key, None);
    assert!(single.matches);
    assert!(!tracker.record_updated(0, single.row_key.as_ref(), true));

    // A partition lookup error falls back to the plain handle so the write path can report it.
    let lookup_failed = tracker.prepare(9, Some(&3), true, PartitionRowLocation::LookupFailed);
    assert_eq!(lookup_failed.row_key, Some(UpdateRowKey::Handle(3)));

    // An unmatched outer-join row has no handle and remains untracked.
    let unmatched = tracker.prepare(
        11,
        None,
        true,
        PartitionRowLocation::NotPartitionedOrClustered,
    );
    assert_eq!(unmatched.row_key, None);
    assert!(unmatched.matches);
}

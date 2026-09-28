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

use crate::mock_region::{GcError, GcStatesManager};

#[test]
fn mock_gc_states_manager_matches_go_contract() {
    let manager = GcStatesManager::default();

    // Go covers NullKeyspaceID, the default keyspace, and a regular keyspace.
    for keyspace_id in [u32::MAX, 0, 1] {
        let state = manager.state(keyspace_id, true);
        assert_eq!(keyspace_id, state.keyspace_id);
        assert_eq!((0, 0), (state.gc_safe_point, state.txn_safe_point));
        assert!(state.barriers.is_empty());

        // An excluded barrier list is represented by an empty vec in the Rust view.
        let state = manager.state(keyspace_id, false);
        assert_eq!((0, 0), (state.gc_safe_point, state.txn_safe_point));
        assert!(state.barriers.is_empty());

        let txn = manager.advance_txn_safe_point(keyspace_id, 10).unwrap();
        assert_eq!(
            (0, 10, 10, None),
            (txn.old, txn.target, txn.new, txn.blocker)
        );
        let gc = manager.advance_gc_safe_point(keyspace_id, 9).unwrap();
        assert_eq!((0, 9, 9, None), (gc.old, gc.target, gc.new, gc.blocker));

        assert_eq!(
            GcError::DecreasingSafePoint {
                current: 10,
                target: 9
            },
            manager.advance_txn_safe_point(keyspace_id, 9).unwrap_err()
        );
        assert_eq!(
            GcError::DecreasingSafePoint {
                current: 9,
                target: 8
            },
            manager.advance_gc_safe_point(keyspace_id, 8).unwrap_err()
        );
        assert_eq!(
            GcError::GcAheadOfTxn { gc: 11, txn: 10 },
            manager.advance_gc_safe_point(keyspace_id, 11).unwrap_err()
        );

        let state = manager.state(keyspace_id, true);
        assert_eq!((9, 10), (state.gc_safe_point, state.txn_safe_point));
        assert!(state.barriers.is_empty());

        assert_eq!(
            GcError::InvalidArguments,
            manager
                .set_barrier(keyspace_id, String::new(), 20)
                .unwrap_err()
        );
        assert_eq!(
            GcError::InvalidArguments,
            manager
                .set_barrier(keyspace_id, "b1".into(), 0)
                .unwrap_err()
        );
        assert_eq!(
            GcError::BarrierBehindTxnSafePoint {
                barrier: 9,
                txn_safe_point: 10
            },
            manager
                .set_barrier(keyspace_id, "b1".into(), 9)
                .unwrap_err()
        );

        let barrier = manager.set_barrier(keyspace_id, "b2".into(), 25).unwrap();
        assert_eq!(("b2", 25), (barrier.id.as_str(), barrier.barrier_ts));

        let state = manager.state(keyspace_id, true);
        assert_eq!((9, 10), (state.gc_safe_point, state.txn_safe_point));
        assert_eq!(1, state.barriers.len());
        assert_eq!(
            ("b2", 25),
            (state.barriers[0].id.as_str(), state.barriers[0].barrier_ts)
        );
        assert!(manager.state(keyspace_id, false).barriers.is_empty());

        let txn = manager.advance_txn_safe_point(keyspace_id, 30).unwrap();
        assert_eq!((10, 30, 25), (txn.old, txn.target, txn.new));
        assert!(txn.blocker.as_deref().unwrap().contains("b2"));
        assert_eq!(25, manager.state(keyspace_id, true).txn_safe_point);

        let barrier = manager.delete_barrier(keyspace_id, "b2").unwrap();
        assert_eq!(("b2", 25), (barrier.id.as_str(), barrier.barrier_ts));

        let txn = manager.advance_txn_safe_point(keyspace_id, 30).unwrap();
        assert_eq!(
            (25, 30, 30, None),
            (txn.old, txn.target, txn.new, txn.blocker)
        );
        let state = manager.state(keyspace_id, true);
        assert_eq!((9, 30), (state.gc_safe_point, state.txn_safe_point));
        assert!(state.barriers.is_empty());
    }
}

#[test]
fn minimum_barrier_wins_and_cannot_move_txn_safe_point_backwards() {
    let manager = GcStatesManager::default();
    manager.advance_txn_safe_point(7, 10).unwrap();
    manager.set_barrier(7, "later".into(), 25).unwrap();
    manager.set_barrier(7, "earlier".into(), 20).unwrap();

    let advanced = manager.advance_txn_safe_point(7, 30).unwrap();
    assert_eq!((10, 30, 20), (advanced.old, advanced.target, advanced.new));
    assert!(advanced.blocker.as_deref().unwrap().contains("earlier"));

    manager.delete_barrier(7, "earlier");
    manager.set_barrier(7, "at-current".into(), 20).unwrap();
    let advanced = manager.advance_txn_safe_point(7, 30).unwrap();
    assert_eq!((20, 30, 20), (advanced.old, advanced.target, advanced.new));
}

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

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::adaptive_limit_controller::{AdaptiveLimitConfig, AdaptiveLimitController};

fn controller(
    demand_rows: u64,
    initial_outer_window: u64,
    max_outer_window: u64,
    initial_lookup_window: u64,
    max_lookup_window: u64,
) -> AdaptiveLimitController {
    AdaptiveLimitController::NewAdaptiveLimitController(AdaptiveLimitConfig {
        demand_rows,
        initial_outer_window,
        max_outer_window,
        initial_lookup_window,
        max_lookup_window,
        initial_lookup_batch_size: initial_lookup_window,
        max_lookup_batch_size: max_lookup_window,
    })
}

#[test]
fn uses_current_execution_yield_and_tapers_headroom() {
    let c = controller(1000, 32, 100_000, 32, 100_000);
    let (reserved, admitted) = c.ReserveOuter(25_000);
    assert!(admitted);
    assert_eq!(reserved, 32);
    c.CommitOuter(reserved, reserved);
    c.ObserveJoinProgress(32, 32);
    assert_eq!(c.ReserveOuter(25_000), (64, true));

    let tail = controller(1000, 1024, 100_000, 1024, 100_000);
    let (reserved, _) = tail.ReserveOuter(25_000);
    assert_eq!(reserved, 1000);
    tail.CommitOuter(reserved, reserved);
    tail.ObserveJoinProgress(999, 999);
    assert_eq!(tail.Snapshot().outer_window, 1);

    let mid = controller(1000, 500, 100_000, 500, 100_000);
    let (reserved, _) = mid.ReserveOuter(25_000);
    mid.CommitOuter(reserved, reserved);
    mid.ObserveJoinProgress(500, 500);
    assert_eq!(mid.Snapshot().outer_window, 563);

    let phase = controller(1000, 1000, 100_000, 1000, 100_000);
    let (reserved, _) = phase.ReserveOuter(1000);
    phase.CommitOuter(reserved, reserved);
    phase.ObserveJoinProgress(900, 900);
    phase.ObserveJoinProgress(100, 10);
    assert_eq!(phase.Snapshot().outer_window, 99);
}

#[test]
fn allows_one_growth_per_progress_epoch() {
    let c = controller(1000, 32, 100_000, 32, 100_000);
    let (reserved, _) = c.ReserveOuter(32);
    c.CommitOuter(reserved, reserved);
    c.ObserveJoinProgress(1, 1);
    assert_eq!(c.Snapshot().outer_window, 64);
    c.ObserveJoinProgress(1, 1);
    c.ObserveJoinProgress(1, 1);
    assert_eq!(c.Snapshot().outer_window, 64);
    let (reserved, _) = c.ReserveOuter(32);
    c.CommitOuter(reserved, reserved);
    c.ObserveJoinProgress(29, 29);
    assert_eq!(c.Snapshot().outer_window, 64);
    c.ObserveJoinProgress(1, 1);
    assert_eq!(c.Snapshot().outer_window, 128);

    let lookup = controller(1000, 32, 100_000, 32, 100_000);
    let (reserved, _) = lookup.ReserveLookup(32);
    lookup.CompleteLookup(reserved, reserved, 1);
    assert_eq!(lookup.Snapshot().lookup_window, 64);
    let (reserved, _) = lookup.ReserveOuter(32);
    lookup.CommitOuter(reserved, reserved);
    lookup.ObserveJoinProgress(1, 1);
    assert_eq!(lookup.Snapshot().lookup_window, 64);
    let (reserved, _) = lookup.ReserveLookup(64);
    lookup.CompleteLookup(reserved, reserved, 1);
    assert_eq!(lookup.Snapshot().lookup_window, 128);
}

#[test]
fn pairs_output_with_completed_outer_rows() {
    let c = controller(1000, 32, 100_000, 32, 100_000);
    let (reserved, _) = c.ReserveOuter(32);
    c.CommitOuter(reserved, reserved);
    for _ in 0..4 {
        c.ObserveJoinProgress(0, 8);
    }
    assert_eq!(c.Snapshot().output_rows, 32);
    assert_eq!(c.Snapshot().outer_window, 32);
    c.ObserveJoinProgress(1, 0);
    assert_eq!(c.Snapshot().outer_consumed, 1);
    assert_eq!(c.Snapshot().outer_window, 39);
}

#[test]
fn grows_when_consumed_input_has_no_output() {
    let c = controller(1000, 32, 100_000, 32, 100_000);
    for expected in [32, 64, 128] {
        let (reserved, _) = c.ReserveOuter(25_000);
        assert_eq!(reserved, expected);
        c.CommitOuter(reserved, reserved);
        c.ObserveJoinProgress(reserved, 0);
    }
    let sparse = controller(1000, 1000, 100_000, 1000, 100_000);
    let (reserved, _) = sparse.ReserveOuter(1000);
    sparse.CommitOuter(reserved, reserved);
    sparse.ObserveJoinProgress(999, 999);
    sparse.ObserveJoinProgress(1, 0);
    assert_eq!(sparse.Snapshot().outer_window, 2);
    for expected in [4, 8] {
        let (reserved, _) = sparse.ReserveOuter(1000);
        sparse.CommitOuter(reserved, reserved);
        sparse.ObserveJoinProgress(reserved, 0);
        assert_eq!(sparse.Snapshot().outer_window, expected);
    }
}

#[test]
fn stop_interrupts_reservation_and_reset_restores_windows() {
    let c = Arc::new(controller(1000, 32, 100_000, 32, 100_000));
    let (outer, _) = c.ReserveOuter(32);
    c.CommitOuter(outer, outer);
    let (lookup, _) = c.ReserveLookup(32);
    let waiter = Arc::clone(&c);
    let join = thread::spawn(move || waiter.ReserveOuter(32));
    thread::sleep(Duration::from_millis(20));
    c.Stop();
    assert_eq!(join.join().unwrap(), (0, false));
    let snapshot = c.Snapshot();
    assert!(snapshot.stopped);
    assert_eq!(snapshot.outer_outstanding_at_stop, outer as u64);
    assert_eq!(snapshot.lookup_outstanding_at_stop, lookup as u64);
    assert!(snapshot.outer_admission_blocked > Duration::ZERO);
    c.Reset();
    assert_eq!(c.ReserveOuter(32), (32, true));
    assert!(!c.Snapshot().stopped);
}

#[test]
fn bounds_lookup_admission_and_rounds_to_execution_batches() {
    let c = AdaptiveLimitController::NewAdaptiveLimitController(AdaptiveLimitConfig {
        demand_rows: 1,
        initial_outer_window: 1,
        max_outer_window: 100_000,
        initial_lookup_window: 1,
        max_lookup_window: 100_000,
        initial_lookup_batch_size: 1024,
        max_lookup_batch_size: 20_000,
    });
    let snapshot = c.Snapshot();
    assert_eq!(snapshot.lookup_window, 1);
    assert_eq!(snapshot.lookup_batch_size, 1024);
    assert_eq!(snapshot.lookup_physical_window, 1024);
    let (reserved, _) = c.ReserveLookup(20_000);
    assert_eq!(reserved, 1024);
    c.CompleteLookup(reserved, reserved, 0);
    let snapshot = c.Snapshot();
    assert_eq!(snapshot.lookup_window, 2);
    assert_eq!(snapshot.lookup_batch_size, 2048);
    assert_eq!(snapshot.lookup_physical_window, 2048);

    let yield_c = controller(1000, 64, 100_000, 32, 100_000);
    let (reserved, _) = yield_c.ReserveLookup(32);
    yield_c.CompleteLookup(reserved, reserved, reserved);
    let (reserved, _) = yield_c.ReserveLookup(32);
    yield_c.CompleteLookup(reserved, reserved, 4);
    let snapshot = yield_c.Snapshot();
    assert_eq!(snapshot.lookup_window, 50);
    assert_eq!(snapshot.lookup_batch_size, 32);
    assert_eq!(snapshot.lookup_physical_window, 64);
}

#[test]
fn direct_lookup_uses_lookup_yield_and_stops_at_demand() {
    let c = AdaptiveLimitController::NewAdaptiveLimitLookupController(AdaptiveLimitConfig {
        demand_rows: 1000,
        initial_lookup_window: 32,
        max_lookup_window: 100_000,
        initial_lookup_batch_size: 32,
        max_lookup_batch_size: 100_000,
        ..AdaptiveLimitConfig::default()
    });
    assert_eq!(c.ReserveOuter(32), (0, false));
    let (reserved, _) = c.ReserveLookup(32);
    c.CompleteLookup(reserved, reserved, 1);
    let snapshot = c.Snapshot();
    assert_eq!(snapshot.output_rows, 1);
    assert_eq!(snapshot.lookup_handles, 32);
    assert!(snapshot.lookup_window > 32);

    let stop = AdaptiveLimitController::NewAdaptiveLimitLookupController(AdaptiveLimitConfig {
        demand_rows: 2,
        initial_lookup_window: 2,
        max_lookup_window: 32,
        initial_lookup_batch_size: 2,
        max_lookup_batch_size: 32,
        ..AdaptiveLimitConfig::default()
    });
    let (reserved, _) = stop.ReserveLookup(2);
    stop.CompleteLookup(reserved, reserved, 2);
    assert!(stop.Snapshot().stopped);
    assert_eq!(stop.Snapshot().lookup_batch_size, 0);
}

#[test]
fn direct_lookup_grows_on_no_output() {
    let c = AdaptiveLimitController::NewAdaptiveLimitLookupController(AdaptiveLimitConfig {
        demand_rows: 1000,
        initial_lookup_window: 32,
        max_lookup_window: 100_000,
        initial_lookup_batch_size: 32,
        max_lookup_batch_size: 100_000,
        ..AdaptiveLimitConfig::default()
    });
    for expected in [64, 128, 256] {
        let (reserved, _) = c.ReserveLookup(1000);
        c.CompleteLookup(reserved, reserved, 0);
        assert_eq!(c.Snapshot().lookup_window, expected);
    }
}

#[test]
fn merges_overlapping_lookup_block_intervals() {
    let c = Arc::new(controller(1000, 1, 1, 1, 1));
    let (reserved, _) = c.ReserveLookup(1);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut joins = Vec::new();
    for _ in 0..2 {
        let waiter = Arc::clone(&c);
        let tx = tx.clone();
        joins.push(thread::spawn(move || {
            tx.send(waiter.ReserveLookup(1)).unwrap();
        }));
    }
    thread::sleep(Duration::from_millis(20));
    c.AbortLookup(reserved);
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), (1, true));
    c.AbortLookup(1);
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), (1, true));
    c.AbortLookup(1);
    for join in joins {
        join.join().unwrap();
    }
    let snapshot = c.Snapshot();
    assert!(snapshot.lookup_admission_blocked > Duration::ZERO);
    assert_eq!(snapshot.lookup_reserved, 0);
}

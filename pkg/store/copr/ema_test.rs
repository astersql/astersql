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

use crate::ema::RuEma;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[test]
fn go_commit_ab7d93b603_ema_seed_and_converge() {
    let now = Instant::now();
    let seeded = RuEma::new(4 * 1024 * 1024);
    assert_eq!(seeded.predict(), 4 * 1024 * 1024);
    seeded.observe(1_000_000, now);
    assert_eq!(seeded.predict(), 1_000_000);
    let unseeded = RuEma::new(0);
    assert_eq!(unseeded.predict(), 0);
    unseeded.observe(1_000_000, now);
    assert_eq!(unseeded.predict(), 1_000_000);
    unseeded.observe(1_000_000, now + Duration::from_millis(100));
    assert!(unseeded.predict().abs_diff(1_000_000) <= 1);
}

#[test]
fn go_commit_ab7d93b603_ema_tracks_shift() {
    let ema = RuEma::new(0);
    let now = Instant::now();
    for i in 0..5 {
        ema.observe(100_000, now + Duration::from_millis(i * 100));
    }
    assert!(ema.predict().abs_diff(100_000) <= 1);
    for i in 5..20 {
        ema.observe(500_000, now + Duration::from_millis(i * 100));
    }
    assert!(ema.predict() > 400_000);
    assert!(ema.predict() <= 500_000);
}

#[test]
fn go_commit_ab7d93b603_ema_large_gap_collapses_weight() {
    let ema = RuEma::new(0);
    let now = Instant::now();
    ema.observe(100_000, now);
    ema.observe(1_000_000, now + Duration::from_secs(10));
    assert!(ema.predict().abs_diff(1_000_000) <= 1_000);
}

#[test]
fn go_commit_ab7d93b603_ema_non_monotonic_time() {
    let ema = RuEma::new(0);
    let now = Instant::now();
    ema.observe(100_000, now);
    ema.observe(500_000, now - Duration::from_secs(1));
    assert!(ema.predict().abs_diff(100_000) <= 1);
    // The stale sample must not rewind the clock used by the next observation.
    ema.observe(500_000, now + Duration::from_millis(100));
    let expected = 100_000.0 + (1.0 - (-0.1_f64).exp()) * 400_000.0;
    assert!(ema.predict().abs_diff(expected as u64) <= 1);
    let unchanged = ema.predict();
    ema.observe(0, now + Duration::from_millis(100));
    assert_eq!(ema.predict(), unchanged);
}

#[test]
fn go_commit_ab7d93b603_ema_concurrent_observe_and_predict() {
    let ema = Arc::new(RuEma::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let reader_ema = ema.clone();
    let reader_done = done.clone();
    let reader = std::thread::spawn(move || {
        while !reader_done.load(Ordering::Acquire) {
            let _ = reader_ema.predict();
        }
    });
    let base = Instant::now();
    let writers: Vec<_> = (0..8)
        .map(|id| {
            let ema = ema.clone();
            std::thread::spawn(move || {
                for i in 0..200 {
                    ema.observe(100_000 + id * 1_000 + i, base + Duration::from_millis(i));
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().expect("EMA writer panicked");
    }
    done.store(true, Ordering::Release);
    reader.join().expect("EMA reader panicked");
    assert!(ema.predict() > 0);
}

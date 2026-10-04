// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use crate::localhelper::{
    BatchSplitClient, CompactionLowerThreshold, CompactionUpperThreshold,
    EstimateCompactionThreshold2, StoreWriteLimiter, beforeEnd, calculateLimitAndBurst,
    coarseGrainedSplitKeysThreshold, getCoarseGrainedSplitKeys, insideRegion, keyInsideRegion,
    largerStartKey, newStoreWriteLimiter, splitAndScatterRegionInBatches,
};
use crate::{CancellationToken, Error, KeyRange, Result};

#[derive(Default)]
struct TestSplitClient {
    batches: Mutex<Vec<Vec<Vec<u8>>>>,
    fail_on_call: Option<usize>,
    cancel_after_call: Option<(usize, CancellationToken)>,
    calls: AtomicUsize,
}

impl BatchSplitClient for TestSplitClient {
    fn SplitAndScatter(&self, token: &CancellationToken, keys: &[Vec<u8>]) -> Result<()> {
        token.check()?;
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.batches.lock().unwrap().push(keys.to_vec());
        if self.fail_on_call == Some(call) {
            return Err(Error::Retryable("mock split error".into()));
        }
        if let Some((cancel_on, token)) = &self.cancel_after_call
            && *cancel_on == call
        {
            token.cancel();
        }
        Ok(())
    }
}

fn make_split_keys(n: usize) -> Vec<Vec<u8>> {
    (0..n).map(|i| vec![(i >> 8) as u8, i as u8]).collect()
}

#[test]
fn store_write_limiter_matches_go_burst_chunking_and_store_isolation() {
    let token = CancellationToken::default();
    let max_limiter = newStoreWriteLimiter(isize::MAX);
    assert_eq!(max_limiter.WaitN(&token, 1, 1024), Ok(()));

    let limiter = newStoreWriteLimiter(100);
    let start = Instant::now();
    assert_eq!(limiter.WaitN(&token, 1, 240), Ok(()));
    assert!(start.elapsed() >= Duration::from_millis(1100));

    let start = Instant::now();
    assert_eq!(limiter.WaitN(&token, 2, 120), Ok(()));
    assert!(start.elapsed() < Duration::from_millis(100));
}

#[test]
fn tune_store_write_limiter_updates_existing_buckets_and_disables_limiting() {
    let token = CancellationToken::default();
    let limiter = newStoreWriteLimiter(100);
    assert_eq!(limiter.WaitN(&token, 1, 120), Ok(()));

    limiter.UpdateLimit(200);
    assert_eq!(limiter.Limit(), 200);
    let start = Instant::now();
    assert_eq!(limiter.WaitN(&token, 1, 120), Ok(()));
    assert!(start.elapsed() >= Duration::from_millis(500));

    limiter.UpdateLimit(0);
    assert_eq!(limiter.Limit(), 0);
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert_eq!(limiter.WaitN(&cancelled, 1, 10_000), Ok(()));
}

#[test]
fn disabling_store_write_limiter_while_creating_bucket_does_not_publish_stale_bucket() {
    let limiter = Arc::new(newStoreWriteLimiter(100));
    let reached_before_write = Arc::new(Barrier::new(2));
    let continue_get_limiter = Arc::new(Barrier::new(2));

    let worker = {
        let limiter = Arc::clone(&limiter);
        let reached_before_write = Arc::clone(&reached_before_write);
        let continue_get_limiter = Arc::clone(&continue_get_limiter);
        std::thread::spawn(move || {
            limiter
                .getLimiterForTest(1, || {
                    reached_before_write.wait();
                    continue_get_limiter.wait();
                })
                .unwrap()
                .is_none()
        })
    };

    reached_before_write.wait();
    limiter.UpdateLimit(0);
    continue_get_limiter.wait();

    assert!(worker.join().unwrap());
    assert_eq!(limiter.limiterCount(), 0);
}

#[test]
fn split_and_scatter_runs_coarse_then_fine_batches() {
    let client = TestSplitClient::default();
    splitAndScatterRegionInBatches(
        &client,
        &CancellationToken::default(),
        &make_split_keys(121),
        50,
        0.0,
    )
    .unwrap();

    let batches = client.batches.lock().unwrap();
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        [12, 50, 50, 21]
    );
    assert_eq!(batches[0], getCoarseGrainedSplitKeys(&make_split_keys(121)));
}

#[test]
fn split_and_scatter_small_input_skips_coarse_layer() {
    let client = TestSplitClient::default();
    splitAndScatterRegionInBatches(
        &client,
        &CancellationToken::default(),
        &make_split_keys(coarseGrainedSplitKeysThreshold),
        50,
        0.0,
    )
    .unwrap();
    assert_eq!(client.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn split_and_scatter_stops_after_coarse_error() {
    let client = TestSplitClient {
        fail_on_call: Some(1),
        ..Default::default()
    };
    assert_eq!(
        splitAndScatterRegionInBatches(
            &client,
            &CancellationToken::default(),
            &make_split_keys(121),
            50,
            0.0,
        ),
        Err(Error::Retryable("mock split error".into()))
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn split_and_scatter_propagates_cancellation_before_next_batch() {
    let token = CancellationToken::default();
    let client = TestSplitClient {
        cancel_after_call: Some((1, token.clone())),
        ..Default::default()
    };
    assert_eq!(
        splitAndScatterRegionInBatches(&client, &token, &make_split_keys(121), 50, 0.0),
        Err(Error::Cancelled)
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn coarse_keys_include_the_last_key_exactly_once() {
    for n in [65, 121, 122] {
        let split_keys = make_split_keys(n);
        let coarse = getCoarseGrainedSplitKeys(&split_keys);
        assert_eq!(
            coarse
                .iter()
                .filter(|key| *key == split_keys.last().unwrap())
                .count(),
            1
        );
    }
}

#[test]
fn region_boundaries_match_go_half_open_ranges() {
    let region = KeyRange {
        start: b"b".to_vec(),
        end: b"f".to_vec(),
    };
    assert!(!beforeEnd(b"f", b"f"));
    assert!(beforeEnd(b"anything", b""));
    assert!(keyInsideRegion(&region, b"b"));
    assert!(!keyInsideRegion(&region, b"f"));
    assert!(insideRegion(
        &region,
        &[KeyRange {
            start: b"b".to_vec(),
            end: b"e".to_vec(),
        }]
    ));
    assert!(!insideRegion(
        &region,
        &[KeyRange {
            start: b"b".to_vec(),
            end: b"f".to_vec(),
        }]
    ));
    assert_eq!(largerStartKey(b"a", b"b"), b"b");
}

#[test]
fn limit_and_compaction_calculations_cover_go_boundaries() {
    assert_eq!(calculateLimitAndBurst(-1), (0, 0));
    assert_eq!(calculateLimitAndBurst(100), (100, 120));
    assert_eq!(calculateLimitAndBurst(isize::MAX), (isize::MAX, isize::MAX));
    assert_eq!(EstimateCompactionThreshold2(0), CompactionLowerThreshold);
    assert_eq!(
        EstimateCompactionThreshold2(CompactionLowerThreshold * 512 * 3 / 2),
        CompactionLowerThreshold * 2
    );
    assert_eq!(
        EstimateCompactionThreshold2(i64::MAX),
        CompactionUpperThreshold
    );
}

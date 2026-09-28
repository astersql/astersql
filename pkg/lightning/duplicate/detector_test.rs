// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Detector 集成单测：大规模并发写入与失败路径。
//
// 使用磁盘外排验证重复组收集、key_id 有序性，以及 Handler 构造失败时错误传播。

use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, mpsc};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::lightning::log::log::L;
use super::util::extsort::disk_sorter::{DiskSorterOptions, open_disk_sorter};
use super::util::extsort::external_sorter::{Error, ExternalSorter};

use super::{DetectOptions, Handler, HandlerConstructor, new_detector};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 测试收集到的一个重复组：用户键及其全部 key_id。
struct ResultRecord {
    key: Vec<u8>,
    key_ids: Vec<Vec<u8>>,
}

/// 将 Handler 回调汇聚并通过 channel 发回主线程的收集器。
struct Collector {
    key: Vec<u8>,
    key_ids: Vec<Vec<u8>>,
    result_tx: mpsc::SyncSender<ResultRecord>,
}

impl Handler for Collector {
    fn begin(&mut self, key: &[u8]) -> Result<(), Error> {
        self.key = key.to_vec();
        Ok(())
    }

    fn append(&mut self, key_id: &[u8]) -> Result<(), Error> {
        self.key_ids.push(key_id.to_vec());
        Ok(())
    }

    fn end(&mut self) -> Result<(), Error> {
        self.result_tx.send(ResultRecord {
            key: std::mem::take(&mut self.key),
            key_ids: std::mem::take(&mut self.key_ids),
        })?;
        Ok(())
    }

    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
/// 多 adder 并发写入随机键，detect 后校验重复组与源数据一致。
fn test_detector() {
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();

    // 10 万键、10 个 adder 按取模分片写入，制造可控重复。
    const NUM_KEYS: usize = 100_000;
    const NUM_ADDERS: usize = 10;

    let mut keys = Vec::with_capacity(NUM_KEYS);
    let mut rng = StdRng::seed_from_u64(0);
    for _ in 0..NUM_KEYS {
        keys.push(rng.gen_range(0..NUM_KEYS as u64).to_be_bytes().to_vec());
    }

    std::thread::scope(|scope| {
        for adder_index in 0..NUM_ADDERS {
            let detector = &detector;
            let ctx = &ctx;
            let keys = &keys;
            scope.spawn(move || {
                let mut adder = detector.key_adder(ctx).unwrap();
                for (key_index, key) in keys.iter().enumerate() {
                    if key_index % NUM_ADDERS == adder_index {
                        adder.add(key, &(key_index as u64).to_be_bytes()).unwrap();
                    }
                }
                adder.close().unwrap();
            });
        }
    });

    let (result_tx, result_rx) = mpsc::sync_channel(keys.len());
    let constructor: HandlerConstructor = {
        let result_tx = result_tx.clone();
        Arc::new(move |_| {
            Ok(Box::new(Collector {
                key: Vec::new(),
                key_ids: Vec::new(),
                result_tx: result_tx.clone(),
            }))
        })
    };
    let (num_dups, result) = detector.detect(
        &ctx,
        Some(&mut DetectOptions {
            concurrency: 4,
            handler_constructor: Some(constructor),
        }),
    );
    result.unwrap();
    sorter.close().unwrap();

    drop(result_tx);
    let mut results: Vec<_> = result_rx.into_iter().collect();
    assert_eq!(results.len(), num_dups as usize);
    verify_results(&keys, &mut results);
}

/// 对照原始 keys 校验：每个重复键至少 2 个 key_id、有序，且 key_id 回指正确。
fn verify_results(keys: &[Vec<u8>], results: &mut Vec<ResultRecord>) {
    for result in results.iter() {
        assert!(
            result.key_ids.len() >= 2,
            "keyIDs should have at least 2 elements"
        );
        assert!(
            result.key_ids.windows(2).all(|pair| pair[0] <= pair[1]),
            "keyIDs should be sorted"
        );
    }
    results.sort_by(|left, right| left.key.cmp(&right.key));

    let mut sorted_keys = keys.to_vec();
    sorted_keys.sort();

    let mut result_index = 0;
    let mut key_index = 0;
    while key_index < sorted_keys.len() {
        let mut next_key_index = key_index + 1;
        while next_key_index < sorted_keys.len()
            && sorted_keys[key_index] == sorted_keys[next_key_index]
        {
            next_key_index += 1;
        }
        if next_key_index - key_index > 1 {
            let key = &sorted_keys[key_index];
            let result = results
                .get(result_index)
                .unwrap_or_else(|| panic!("missing result for duplicated key {key:?}"));
            assert_eq!(key, &result.key, "duplicate key mismatch");
            assert_eq!(
                result.key_ids.len(),
                next_key_index - key_index,
                "duplicate keyIDs mismatch"
            );
            for key_id in &result.key_ids {
                let bytes: [u8; 8] = key_id.as_slice().try_into().unwrap();
                let original_index = u64::from_be_bytes(bytes) as usize;
                assert_eq!(key, &keys[original_index], "keyID refers to wrong key");
            }
            result_index += 1;
        }
        key_index = next_key_index;
    }
    assert_eq!(result_index, results.len(), "unexpected results");
}

#[derive(Clone)]
/// 可按 Arc 指针同一性断言的模拟错误（验证错误未被包装替换）。
struct MockError(Arc<()>);

impl fmt::Debug for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mock error")
    }
}

impl fmt::Display for MockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mock error")
    }
}

impl StdError for MockError {}

#[test]
/// HandlerConstructor 返回错误时，detect 应原样返回该错误且不挂起。
fn test_detector_fail() {
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();

    let mut adder = detector.key_adder(&ctx).unwrap();
    adder.add(b"key", b"keyID").unwrap();
    adder.close().unwrap();

    let mock_error = MockError(Arc::new(()));
    let expected_identity = mock_error.0.clone();
    let constructor: HandlerConstructor = Arc::new(move |_| Err(Box::new(mock_error.clone())));
    let error = detector
        .detect(
            &ctx,
            Some(&mut DetectOptions {
                concurrency: 4,
                handler_constructor: Some(constructor),
            }),
        )
        .1
        .unwrap_err();
    let actual = error
        .downcast_ref::<MockError>()
        .expect("Detect should return the handler constructor error");
    assert!(Arc::ptr_eq(&actual.0, &expected_identity));
    sorter.close().unwrap();
}

#[test]
fn test_detector_cancels_worker_context_after_success() {
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();
    let mut adder = detector.key_adder(&ctx).unwrap();
    adder.add(b"key", b"one").unwrap();
    adder.add(b"key", b"two").unwrap();
    adder.flush().unwrap();
    adder.close().unwrap();
    let (context_tx, context_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let constructor: HandlerConstructor = Arc::new(move |ctx| {
        context_tx.send(ctx.clone()).unwrap();
        Ok(Box::new(Collector {
            key: Vec::new(),
            key_ids: Vec::new(),
            result_tx: result_tx.clone(),
        }))
    });
    let (count, result) = detector.detect(
        &ctx,
        Some(&mut DetectOptions {
            concurrency: 1,
            handler_constructor: Some(constructor),
        }),
    );
    result.unwrap();
    assert_eq!(count, 1);
    assert!(
        context_rx.recv().unwrap().is_cancelled(),
        "errgroup.Wait cancels its context even on success"
    );
    assert!(
        !ctx.is_cancelled(),
        "the caller's context must remain active"
    );
    assert_eq!(
        result_rx.recv().unwrap().key_ids,
        vec![b"one".to_vec(), b"two".to_vec()]
    );
    sorter.close().unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureAt {
    NewWriter,
    Put,
    Flush,
    WriterClose,
    Sort,
    NewIterator,
    First,
    Last,
    Seek,
    Next,
    Begin,
    Append,
    End,
    Close,
}

struct FaultSorter {
    inner: Arc<dyn ExternalSorter>,
    at: FailureAt,
    error: MockError,
    closed: Arc<std::sync::atomic::AtomicUsize>,
}
impl ExternalSorter for FaultSorter {
    fn new_writer(
        &self,
        ctx: &CancellationToken,
    ) -> Result<Box<dyn super::util::extsort::Writer>, Error> {
        if self.at == FailureAt::NewWriter {
            return Err(Box::new(self.error.clone()));
        }
        Ok(Box::new(FaultWriter {
            inner: self.inner.new_writer(ctx)?,
            at: self.at,
            error: self.error.clone(),
        }))
    }
    fn sort(&self, ctx: &CancellationToken) -> Result<(), Error> {
        if self.at == FailureAt::Sort {
            return Err(Box::new(self.error.clone()));
        }
        self.inner.sort(ctx)
    }
    fn is_sorted(&self) -> bool {
        self.inner.is_sorted()
    }
    fn new_iterator(
        &self,
        ctx: &CancellationToken,
    ) -> Result<Box<dyn super::util::extsort::Iterator>, Error> {
        if self.at == FailureAt::NewIterator {
            return Err(Box::new(self.error.clone()));
        }
        Ok(Box::new(FaultIterator {
            inner: self.inner.new_iterator(ctx)?,
            at: self.at,
            error: self.error.clone(),
            pending: None,
            next_calls: 0,
            closed: self.closed.clone(),
        }))
    }
    fn close(&self) -> Result<(), Error> {
        self.inner.close()
    }
    fn close_and_cleanup(&self) -> Result<(), Error> {
        self.inner.close_and_cleanup()
    }
}
struct FaultIterator {
    inner: Box<dyn super::util::extsort::Iterator>,
    at: FailureAt,
    error: MockError,
    pending: Option<Error>,
    next_calls: usize,
    closed: Arc<std::sync::atomic::AtomicUsize>,
}
impl FaultIterator {
    fn fail(&mut self, at: FailureAt) -> bool {
        if self.at == at {
            self.pending = Some(Box::new(self.error.clone()));
            true
        } else {
            false
        }
    }
}
impl super::util::extsort::Iterator for FaultIterator {
    fn seek(&mut self, key: &[u8]) -> bool {
        !self.fail(FailureAt::Seek) && self.inner.seek(key)
    }
    fn first(&mut self) -> bool {
        !self.fail(FailureAt::First) && self.inner.first()
    }
    fn last(&mut self) -> bool {
        !self.fail(FailureAt::Last) && self.inner.last()
    }
    fn next(&mut self) -> bool {
        self.next_calls += 1;
        !(self.next_calls == 2 && self.fail(FailureAt::Next)) && self.inner.next()
    }
    fn valid(&self) -> bool {
        self.pending.is_none() && self.inner.valid()
    }
    fn error(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        self.pending.as_deref().or_else(|| self.inner.error())
    }
    fn take_error(&mut self) -> Option<Error> {
        self.pending.take().or_else(|| self.inner.take_error())
    }
    fn unsafe_key(&self) -> &[u8] {
        self.inner.unsafe_key()
    }
    fn unsafe_value(&self) -> &[u8] {
        self.inner.unsafe_value()
    }
    fn close(&mut self) -> Result<(), Error> {
        self.closed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.close()
    }
}
struct FaultHandler {
    at: FailureAt,
    error: MockError,
    closes: Arc<std::sync::atomic::AtomicUsize>,
}
impl FaultHandler {
    fn check(&self, at: FailureAt) -> Result<(), Error> {
        if self.at == at {
            Err(Box::new(self.error.clone()))
        } else {
            Ok(())
        }
    }
}
impl Handler for FaultHandler {
    fn begin(&mut self, _: &[u8]) -> Result<(), Error> {
        self.check(FailureAt::Begin)
    }
    fn append(&mut self, _: &[u8]) -> Result<(), Error> {
        self.check(FailureAt::Append)
    }
    fn end(&mut self) -> Result<(), Error> {
        self.check(FailureAt::End)
    }
    fn close(&mut self) -> Result<(), Error> {
        self.closes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.check(FailureAt::Close)
    }
}

#[test]
fn test_detector_failure_contracts() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for (at, expected_count, iterator_closes, handler_closes) in [
        (FailureAt::Sort, 0, 0, 0),
        (FailureAt::NewIterator, 0, 0, 0),
        (FailureAt::First, 0, 1, 0),
        (FailureAt::Last, 0, 1, 0),
        (FailureAt::Seek, 0, 2, 1),
        (FailureAt::Next, 1, 2, 1),
        (FailureAt::Begin, 0, 2, 1),
        (FailureAt::Append, 0, 2, 1),
        (FailureAt::End, 1, 2, 1),
        (FailureAt::Close, 2, 2, 1),
    ] {
        let directory = tempdir().unwrap();
        let inner =
            Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
        let identity = MockError(Arc::new(()));
        let closed = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let sorter = Arc::new(FaultSorter {
            inner,
            at,
            error: identity.clone(),
            closed: closed.clone(),
        });
        let detector = new_detector(sorter.clone(), L());
        let ctx = CancellationToken::new();
        let mut adder = detector.key_adder(&ctx).unwrap();
        for key in [b"a", b"b"] {
            for id in [b"1", b"2"] {
                adder.add(key, id).unwrap();
            }
        }
        adder.close().unwrap();
        let constructor: HandlerConstructor = {
            let error = identity.clone();
            let closes = closes.clone();
            Arc::new(move |_| {
                Ok(Box::new(FaultHandler {
                    at,
                    error: error.clone(),
                    closes: closes.clone(),
                }))
            })
        };
        let (count, result) = detector.detect(
            &ctx,
            Some(&mut DetectOptions {
                concurrency: 1,
                handler_constructor: Some(constructor),
            }),
        );
        assert_eq!(count, expected_count, "partial count at {at:?}");
        let error = result.unwrap_err();
        let actual = error
            .downcast_ref::<MockError>()
            .unwrap_or_else(|| panic!("original error lost at {at:?}: {error}"));
        assert!(
            Arc::ptr_eq(&actual.0, &identity.0),
            "error identity at {at:?}"
        );
        assert_eq!(
            closed.load(Ordering::SeqCst),
            iterator_closes,
            "iterator cleanup at {at:?}"
        );
        assert_eq!(
            closes.load(Ordering::SeqCst),
            handler_closes,
            "handler cleanup at {at:?}"
        );
        assert!(!ctx.is_cancelled());
        sorter.close().unwrap();
    }
}

#[test]
fn test_detector_default_concurrency_uses_runtime_setting() {
    // Process-local setting restored even if an assertion panics.
    struct Restore(i64);
    impl Drop for Restore {
        fn drop(&mut self) {
            goish::runtime::GOMAXPROCS(self.0);
        }
    }
    let _restore = Restore(goish::runtime::GOMAXPROCS(3));
    for concurrency in [0, -1, 2] {
        let directory = tempdir().unwrap();
        let sorter =
            Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
        let detector = new_detector(sorter.clone(), L());
        let ctx = CancellationToken::new();
        let mut adder = detector.key_adder(&ctx).unwrap();
        adder.add(b"a", b"1").unwrap();
        adder.close().unwrap();
        let (tx, rx) = mpsc::channel();
        let constructor: HandlerConstructor = Arc::new(move |ctx| {
            tx.send(ctx.clone()).unwrap();
            Ok(Box::new(FaultHandler {
                at: FailureAt::Sort,
                error: MockError(Arc::new(())),
                closes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }))
        });
        let (count, result) = detector.detect(
            &ctx,
            Some(&mut DetectOptions {
                concurrency,
                handler_constructor: Some(constructor),
            }),
        );
        result.unwrap();
        assert_eq!(count, 0);
        let contexts: Vec<_> = rx.into_iter().collect();
        assert_eq!(contexts.len(), if concurrency <= 0 { 3 } else { 2 });
        assert!(contexts.iter().all(CancellationToken::is_cancelled));
        sorter.close().unwrap();
    }
}

#[test]
fn test_detector_empty_and_default_handler() {
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();
    let mut opts = DetectOptions {
        concurrency: -7,
        handler_constructor: None,
    };
    let (count, result) = detector.detect(&ctx, Some(&mut opts));
    result.unwrap();
    assert_eq!(count, 0);
    assert!(opts.concurrency > 0);
    assert!(
        opts.handler_constructor.is_some(),
        "defaults update the caller's options"
    );
    sorter.close().unwrap();

    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let mut adder = detector.key_adder(&ctx).unwrap();
    for key in [b"a".as_slice(), b"a\0", b"\xff"] {
        for id in [b"1", b"2"] {
            adder.add(key, id).unwrap();
        }
    }
    adder.close().unwrap();
    let (count, result) = detector.detect(&ctx, None);
    result.unwrap();
    assert_eq!(count, 3, "default handler and exclusive upper bound");
    sorter.close().unwrap();
}

#[test]
fn test_detector_keeps_root_error_when_other_constructor_observes_cancellation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let directory = tempdir().unwrap();
    let sorter =
        Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
    let detector = new_detector(sorter.clone(), L());
    let ctx = CancellationToken::new();
    let mut adder = detector.key_adder(&ctx).unwrap();
    adder.add(b"a", b"1").unwrap();
    adder.close().unwrap();
    let identity = MockError(Arc::new(()));
    let constructor: HandlerConstructor = {
        let identity = identity.clone();
        let entered = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(2);
        Arc::new(move |ctx| {
            let index = entered.fetch_add(1, Ordering::SeqCst);
            barrier.wait();
            if index == 0 {
                return Err(Box::new(identity.clone()));
            }
            while !ctx.is_cancelled() {
                std::thread::yield_now();
            }
            Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "context canceled").into())
        })
    };
    let (count, result) = detector.detect(
        &ctx,
        Some(&mut DetectOptions {
            concurrency: 2,
            handler_constructor: Some(constructor),
        }),
    );
    assert_eq!(count, 0);
    let error = result.unwrap_err();
    assert!(Arc::ptr_eq(
        &error.downcast_ref::<MockError>().unwrap().0,
        &identity.0
    ));
    assert!(!ctx.is_cancelled());
    sorter.close().unwrap();
}

struct FaultWriter {
    inner: Box<dyn super::util::extsort::Writer>,
    at: FailureAt,
    error: MockError,
}
impl super::util::extsort::Writer for FaultWriter {
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        if self.at == FailureAt::Put {
            return Err(Box::new(self.error.clone()));
        }
        self.inner.put(key, value)
    }
    fn flush(&mut self) -> Result<(), Error> {
        if self.at == FailureAt::Flush {
            return Err(Box::new(self.error.clone()));
        }
        self.inner.flush()
    }
    fn close(&mut self) -> Result<(), Error> {
        self.inner.close()?;
        if self.at == FailureAt::WriterClose {
            return Err(Box::new(self.error.clone()));
        }
        Ok(())
    }
}

#[test]
fn test_key_adder_preserves_writer_errors() {
    for at in [
        FailureAt::NewWriter,
        FailureAt::Put,
        FailureAt::Flush,
        FailureAt::WriterClose,
    ] {
        let directory = tempdir().unwrap();
        let inner =
            Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
        let identity = MockError(Arc::new(()));
        let sorter = Arc::new(FaultSorter {
            inner,
            at,
            error: identity.clone(),
            closed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let detector = new_detector(sorter.clone(), L());
        let ctx = CancellationToken::new();
        let error = match detector.key_adder(&ctx) {
            Err(error) => error,
            Ok(mut adder) => {
                let error = match at {
                    FailureAt::Put => adder.add(b"a", b"1").unwrap_err(),
                    FailureAt::Flush => {
                        adder.add(b"a", b"1").unwrap();
                        adder.flush().unwrap_err()
                    }
                    FailureAt::WriterClose => adder.close().unwrap_err(),
                    _ => panic!("expected writer creation to fail"),
                };
                if at != FailureAt::WriterClose {
                    adder.close().unwrap();
                }
                error
            }
        };
        assert!(
            Arc::ptr_eq(&identity.0, &error.downcast_ref::<MockError>().unwrap().0),
            "{at:?}"
        );
        sorter.close().unwrap();
    }
}

#[test]
fn test_detector_closes_iterator_on_invalid_range_key() {
    for invalid in [b"".as_slice(), b"\xff"] {
        let directory = tempdir().unwrap();
        let inner =
            Arc::new(open_disk_sorter(directory.path(), DiskSorterOptions::default()).unwrap());
        let ctx = CancellationToken::new();
        let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut writer = inner.new_writer(&ctx).unwrap();
        let mut encoded = Vec::new();
        super::encode_internal_key(
            &mut encoded,
            &super::InternalKey::new(b"a".to_vec(), b"1".to_vec()),
        );
        writer.put(&encoded, &[]).unwrap();
        writer.put(invalid, &[]).unwrap();
        writer.close().unwrap();
        let sorter = Arc::new(FaultSorter {
            inner,
            at: FailureAt::Close,
            error: MockError(Arc::new(())),
            closed: closed.clone(),
        });
        let detector = new_detector(sorter.clone(), L());
        let (count, result) = detector.detect(&ctx, None);
        assert_eq!(count, 0);
        let expected =
            super::decode_internal_key(invalid, &mut super::InternalKey::default()).unwrap_err();
        assert_eq!(result.unwrap_err().to_string(), expected.to_string());
        assert_eq!(closed.load(std::sync::atomic::Ordering::SeqCst), 1);
        sorter.close().unwrap();
    }
}

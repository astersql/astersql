// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 外部导入引擎单元测试：覆盖 `MemoryIngestData` 范围查询、重复键策略、多批次加载与释放等待。
//
// 对应 Go `engine_test.go`；Rust 侧 `LoadIngestData` 为同步返回全部批次，无 failpoint。

// Ported from pkg/ingestor/globalsort/engine_test.go. Go's `LoadIngestData`
// streams `engineapi.DataAndRanges` through a channel while a background
// worker pool tunes concurrency live and failpoints inject races
// (`TestLoadRangeBatchDataReleasesReadersWhileWaitingForDownstream`,
// `TestChangeEngineConcurrency`); this port's `Engine::LoadIngestData`
// (engine.rs) is synchronous and returns every batch eagerly, with no
// failpoint hooks. The tests below exercise the same real production
// entry points — `MemoryIngestData`, `NewExternalEngine`/`LoadIngestData`
// duplicate handling, `SetWorkerPool`/`UpdateResource`, and
// `waitIngestDataReleased` — through the APIs this crate actually exposes.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

use crate::engine::{MemoryIngestData, NewExternalEngine, WorkerPoolTuner};
use crate::reader::CancellationToken;
use crate::{Error, KvPair, MemoryStorage, OnDuplicateKey, Storage, encode_kvs};

/// 构造测试用 KV 对。
fn kv(key: &[u8], value: &[u8]) -> KvPair {
    KvPair {
        key: key.to_vec(),
        value: value.to_vec(),
    }
}

/// 解包 `GetFirstAndLastKey`，失败则 panic。
fn get_first_and_last_key(
    data: &MemoryIngestData,
    lower: &[u8],
    upper: &[u8],
) -> (Vec<u8>, Vec<u8>) {
    data.GetFirstAndLastKey(lower, upper)
        .expect("GetFirstAndLastKey should succeed")
}

/// 遍历迭代器收集范围内全部 KV。
fn collect_iter(data: &MemoryIngestData, lower: &[u8], upper: &[u8]) -> Vec<KvPair> {
    let mut iter = data.NewIter(lower, upper).expect("NewIter should succeed");
    let mut out = Vec::new();
    let mut valid = iter.First();
    while valid {
        assert!(iter.Error().is_none());
        out.push(kv(iter.Key(), iter.Value()));
        valid = iter.Next();
    }
    assert!(iter.Error().is_none());
    iter.Close().expect("Close should succeed");
    out
}

// test_memory_ingest_data corresponds to Go's TestMemoryIngestData: it
// checks GetFirstAndLastKey/NewIter bound handling, first over unique keys
// and then over a sequence containing duplicate keys (adjacent, since the
// underlying storage must stay sorted by key).
/// 校验唯一键与含重复键序列上的首尾键/迭代边界行为。
#[test]
fn test_memory_ingest_data() {
    let kvs: Vec<KvPair> = (1..=5)
        .map(|i| kv(format!("key{i}").as_bytes(), format!("value{i}").as_bytes()))
        .collect();
    let data = MemoryIngestData::new(
        kvs.clone(),
        123,
        Arc::new(AtomicI64::new(0)),
        Arc::new(AtomicI64::new(0)),
        || {},
    );

    assert_eq!(123, data.GetTS());
    assert_eq!(
        (b"key1".to_vec(), b"key5".to_vec()),
        get_first_and_last_key(&data, b"", b"")
    );
    assert_eq!(
        (b"key1".to_vec(), b"key5".to_vec()),
        get_first_and_last_key(&data, b"key1", b"key6")
    );
    assert_eq!(
        (b"key2".to_vec(), b"key4".to_vec()),
        get_first_and_last_key(&data, b"key2", b"key5")
    );
    assert_eq!(
        (b"key3".to_vec(), b"key3".to_vec()),
        get_first_and_last_key(&data, b"key25", b"key35")
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        get_first_and_last_key(&data, b"key25", b"key26")
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        get_first_and_last_key(&data, b"key0", b"key1")
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        get_first_and_last_key(&data, b"key6", b"key9")
    );

    assert_eq!(kvs, collect_iter(&data, b"", b""));
    assert_eq!(kvs, collect_iter(&data, b"key1", b"key6"));
    assert_eq!(&kvs[1..4], collect_iter(&data, b"key2", b"key5"));
    assert_eq!(&kvs[2..3], collect_iter(&data, b"key25", b"key35"));
    assert!(collect_iter(&data, b"key25", b"key26").is_empty());
    assert!(collect_iter(&data, b"key0", b"key1").is_empty());
    assert!(collect_iter(&data, b"key6", b"key9").is_empty());

    // Second half: duplicate every odd-indexed key (key2, key4) with an
    // extra, distinguishable value right after it, keeping the sequence
    // sorted by key (required by GetFirstAndLastKey/NewIter's binary
    // search).
    // 后半段：为奇数下标键插入相邻重复项，验证二分边界仍正确。
    let mut encoded_kvs = Vec::with_capacity(kvs.len() * 2);
    for (i, pair) in kvs.iter().enumerate() {
        encoded_kvs.push(pair.clone());
        if i % 2 == 0 {
            continue;
        }
        let mut extra_value = pair.value.clone();
        extra_value.push(1);
        encoded_kvs.push(kv(&pair.key, &extra_value));
    }
    let data = MemoryIngestData::new(
        encoded_kvs,
        234,
        Arc::new(AtomicI64::new(0)),
        Arc::new(AtomicI64::new(0)),
        || {},
    );

    assert_eq!(234, data.GetTS());
    assert_eq!(
        (b"key1".to_vec(), b"key5".to_vec()),
        get_first_and_last_key(&data, b"", b"")
    );
    assert_eq!(
        (b"key1".to_vec(), b"key5".to_vec()),
        get_first_and_last_key(&data, b"key1", b"key6")
    );
    assert_eq!(
        (b"key2".to_vec(), b"key4".to_vec()),
        get_first_and_last_key(&data, b"key2", b"key5")
    );
    assert_eq!(
        (b"key3".to_vec(), b"key3".to_vec()),
        get_first_and_last_key(&data, b"key25", b"key35")
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        get_first_and_last_key(&data, b"key25", b"key26")
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        get_first_and_last_key(&data, b"key0", b"key1")
    );
    assert_eq!(
        (Vec::new(), Vec::new()),
        get_first_and_last_key(&data, b"key6", b"key9")
    );
}

/// 将多组 KV 内容写入内存存储，返回 data/stat 路径列表。
fn write_contents(store: &dyn Storage, contents: &[Vec<KvPair>]) -> (Vec<String>, Vec<String>) {
    let mut data_files = Vec::new();
    let mut stat_files = Vec::new();
    for (index, content) in contents.iter().enumerate() {
        let data_file = format!("/test/{index}.data");
        let stat_file = format!("/test/{index}.stat");
        store.write(&data_file, encode_kvs(content)).unwrap();
        store.write(&stat_file, Vec::new()).unwrap();
        data_files.push(data_file);
        stat_files.push(stat_file);
    }
    (data_files, stat_files)
}

/// 收集批次内全部 KV。
fn get_all_data(data: &MemoryIngestData) -> Vec<KvPair> {
    collect_iter(data, b"", b"")
}

/// 构造用于重复键策略测试的固定参数引擎。
fn dup_engine(
    store: Arc<dyn Storage>,
    data_files: Vec<String>,
    stat_files: Vec<String>,
    on_dup: OnDuplicateKey,
) -> crate::engine::Engine {
    NewExternalEngine(
        store,
        data_files,
        stat_files,
        vec![1],
        vec![5],
        vec![vec![1], vec![2], vec![3], vec![4], vec![5]],
        vec![vec![1], vec![3], vec![5]],
        10,
        123,
        456,
        789,
        true,
        16 * 1024 * 1024 * 1024,
        on_dup,
        "/".to_owned(),
    )
    .expect("NewExternalEngine should succeed")
}

// test_engine_on_dup corresponds to Go's TestEngineOnDup: it checks that
// Ignore/Error abort LoadIngestData on the first duplicate key found (after
// sorting), while Record/Remove drop every occurrence of a repeated key
// (simplesst.RemoveDuplicates semantics, keeping only keys that appear
// exactly once), with Record additionally accumulating ConflictInfo and
// persisting the removed pairs to "<prefix>/dup".
/// 校验 Ignore/Error 遇重复即失败，Record/Remove 整组丢弃，Record 另写 dup 文件。
#[test]
fn test_engine_on_dup() {
    let contents = vec![vec![
        kv(&[4], b"bbb"),
        kv(&[4], b"bbb"),
        kv(&[1], b"aa"),
        kv(&[1], b"aa"),
        kv(&[1], b"aa"),
        kv(&[2], b"vv"),
        kv(&[3], b"sds"),
    ]];

    {
        let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
        let (data_files, stat_files) = write_contents(store.as_ref(), &contents);
        let mut engine = dup_engine(store, data_files, stat_files, OnDuplicateKey::Ignore);
        match engine.LoadIngestData(&CancellationToken::default()) {
            Err(Error::DuplicateKey { .. }) => {}
            other => panic!(
                "Ignore must still report duplicate keys, got {}",
                other.is_ok()
            ),
        }
        engine.Close().unwrap();
    }

    {
        let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
        let (data_files, stat_files) = write_contents(store.as_ref(), &contents);
        let mut engine = dup_engine(store, data_files, stat_files, OnDuplicateKey::Error);
        match engine.LoadIngestData(&CancellationToken::default()) {
            Err(Error::DuplicateKey { key, value }) => {
                assert_eq!(vec![1], key);
                assert_eq!(b"aa".to_vec(), value);
            }
            Err(other) => panic!("expected DuplicateKey, got {other:?}"),
            Ok(_) => panic!("expected DuplicateKey, got Ok"),
        }
        engine.Close().unwrap();
    }

    // 无重复时 Record/Remove 均应保留全部键。
    for on_dup in [OnDuplicateKey::Record, OnDuplicateKey::Remove] {
        let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
        let no_dup_contents = vec![vec![
            kv(&[4], b"bbb"),
            kv(&[1], b"aa"),
            kv(&[2], b"vv"),
            kv(&[3], b"sds"),
        ]];
        let (data_files, stat_files) = write_contents(store.as_ref(), &no_dup_contents);
        let mut engine = dup_engine(store, data_files, stat_files, on_dup);
        let batches = engine
            .LoadIngestData(&CancellationToken::default())
            .expect("no-duplicate load should succeed");
        assert_eq!(1, batches.len());
        assert_eq!(
            vec![
                kv(&[1], b"aa"),
                kv(&[2], b"vv"),
                kv(&[3], b"sds"),
                kv(&[4], b"bbb")
            ],
            get_all_data(&batches[0].data)
        );
        let info = engine.ConflictInfo();
        assert_eq!(0, info.count);
        engine.Close().unwrap();
    }

    let contents2 = vec![
        vec![kv(&[1], b"aa"), kv(&[1], b"aa")],
        vec![kv(&[1], b"aa"), kv(&[2], b"vv"), kv(&[3], b"sds")],
        vec![kv(&[4], b"bbb"), kv(&[4], b"bbb")],
    ];
    for content in [contents.clone(), contents2] {
        for on_dup in [OnDuplicateKey::Record, OnDuplicateKey::Remove] {
            let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
            let (data_files, stat_files) = write_contents(store.as_ref(), &content);
            let mut engine = dup_engine(store.clone(), data_files, stat_files, on_dup);
            let batches = engine
                .LoadIngestData(&CancellationToken::default())
                .expect("partially duplicated load should succeed");
            assert_eq!(1, batches.len());
            // 仅保留出现恰好一次的键 2、3。
            assert_eq!(
                vec![kv(&[2], b"vv"), kv(&[3], b"sds")],
                get_all_data(&batches[0].data)
            );
            let info = engine.ConflictInfo();
            if on_dup == OnDuplicateKey::Remove {
                assert_eq!(0, info.count);
                assert!(info.files.is_empty());
            } else {
                assert_eq!(5, info.count);
                assert_eq!(vec!["/dup"], info.files);
                let dup_bytes = store.read("/dup").expect("dup file should be written");
                let mut dup_pairs = crate::decode_kvs(&dup_bytes, 0).unwrap();
                dup_pairs.sort_by(|a, b| a.key.cmp(&b.key).then(a.value.cmp(&b.value)));
                assert_eq!(
                    vec![
                        kv(&[1], b"aa"),
                        kv(&[1], b"aa"),
                        kv(&[1], b"aa"),
                        kv(&[4], b"bbb"),
                        kv(&[4], b"bbb"),
                    ],
                    dup_pairs
                );
            }
            engine.Close().unwrap();
        }
    }

    // 全部为重复键时输出为空；Record 仍写出全部副本。
    for on_dup in [OnDuplicateKey::Record, OnDuplicateKey::Remove] {
        let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
        let all_dup_contents = vec![vec![
            kv(&[1], b"aaa"),
            kv(&[1], b"aaa"),
            kv(&[1], b"aaa"),
            kv(&[1], b"aaa"),
        ]];
        let (data_files, stat_files) = write_contents(store.as_ref(), &all_dup_contents);
        let mut engine = dup_engine(store.clone(), data_files, stat_files, on_dup);
        let batches = engine
            .LoadIngestData(&CancellationToken::default())
            .expect("all-duplicated load should succeed");
        assert_eq!(1, batches.len());
        assert!(get_all_data(&batches[0].data).is_empty());
        let info = engine.ConflictInfo();
        if on_dup == OnDuplicateKey::Remove {
            assert_eq!(0, info.count);
            assert!(info.files.is_empty());
        } else {
            assert_eq!(4, info.count);
            assert_eq!(vec!["/dup"], info.files);
            let dup_pairs = crate::decode_kvs(&store.read("/dup").unwrap(), 0).unwrap();
            assert_eq!(vec![kv(&[1], b"aaa"); 4], dup_pairs);
        }
        engine.Close().unwrap();
    }
}

// test_load_ingest_data_multi_batch corresponds to Go's
// TestLoadIngestDataMultiBatch: with workerConcurrency=2 and 5 job keys (4
// ranges), LoadIngestData should split the work into exactly two batches
// whose combined data reproduces every written KV pair in order.
/// 并发度为 2、5 个 job 键时应拆成两批，合并结果覆盖全部 KV。
#[test]
fn test_load_ingest_data_multi_batch() {
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let contents = vec![
        vec![
            kv(&[1], b"v1"),
            kv(&[2], b"v2"),
            kv(&[3], b"v3"),
            kv(&[4], b"v4"),
        ],
        vec![
            kv(&[5], b"v5"),
            kv(&[6], b"v6"),
            kv(&[7], b"v7"),
            kv(&[8], b"v8"),
        ],
    ];
    let (data_files, stat_files) = write_contents(store.as_ref(), &contents);

    let mut engine = NewExternalEngine(
        Arc::clone(&store),
        data_files,
        stat_files,
        vec![1],
        vec![9],
        vec![vec![1], vec![3], vec![5], vec![7], vec![9]],
        vec![vec![1], vec![5], vec![9]],
        2,
        123,
        456,
        8,
        true,
        16 * 1024 * 1024 * 1024,
        OnDuplicateKey::Error,
        "/".to_owned(),
    )
    .expect("NewExternalEngine should succeed");

    let batches = engine
        .LoadIngestData(&CancellationToken::default())
        .expect("multi-batch load should succeed");
    assert_eq!(2, batches.len(), "expected 2 batches from LoadIngestData");

    let mut all_kvs = Vec::new();
    for batch in &batches {
        all_kvs.extend(get_all_data(&batch.data));
    }
    assert_eq!(
        vec![
            kv(&[1], b"v1"),
            kv(&[2], b"v2"),
            kv(&[3], b"v3"),
            kv(&[4], b"v4"),
            kv(&[5], b"v5"),
            kv(&[6], b"v6"),
            kv(&[7], b"v7"),
            kv(&[8], b"v8"),
        ],
        all_kvs
    );
    engine.Close().unwrap();
}

/// 记录 `Tune` 调用并发度的假工作池。
struct DummyWorker {
    tuned: AtomicI32,
}

impl WorkerPoolTuner for DummyWorker {
    fn Tune(&self, concurrency: usize) {
        self.tuned.store(concurrency as i32, Ordering::SeqCst);
    }
}

// test_update_resource_and_worker_pool corresponds to the concurrency-tuning
// half of Go's TestChangeEngineConcurrency: an unchanged concurrency is a
// no-op before the memory-capacity argument is inspected; actual changes
// reject non-positive inputs and tune the worker pool.
/// 校验未变并发度优先无操作返回，实际变更才校验参数并调谐工作池。
#[test]
fn test_update_resource_and_worker_pool() {
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let mut engine = NewExternalEngine(
        store,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        4,
        0,
        0,
        0,
        false,
        1024,
        OnDuplicateKey::Error,
        String::new(),
    )
    .expect("NewExternalEngine should succeed");
    assert_eq!("external", engine.ID());

    match engine.UpdateResource(8, 4096) {
        Err(Error::InvalidArgument(message)) => {
            assert!(message.contains("worker"), "unexpected error: {message}");
        }
        other => panic!("expected an uninitialized-worker error, got {other:?}"),
    }

    let worker = Arc::new(DummyWorker {
        tuned: AtomicI32::new(0),
    });
    engine.SetWorkerPool(Arc::clone(&worker) as Arc<dyn WorkerPoolTuner>);

    match engine.UpdateResource(0, 1024) {
        Err(Error::InvalidArgument(_)) => {}
        other => panic!("expected InvalidArgument for zero concurrency, got {other:?}"),
    }
    engine
        .UpdateResource(4, 0)
        .expect("unchanged concurrency should be a no-op before validating memory capacity");
    assert_eq!(0, worker.tuned.load(Ordering::SeqCst));

    engine
        .UpdateResource(8, 4096)
        .expect("positive update should succeed");
    assert_eq!(8, worker.tuned.load(Ordering::SeqCst));
}

// test_wait_ingest_data_released corresponds to the release/retry half of
// Go's TestLoadRangeBatchDataReleasesReadersWhileWaitingForDownstream: with
// no in-flight data, waiting should fail fast (mirroring the Go check that
// short-circuits before blocking); once a batch is loaded, releasing it
// (DecRef) on another thread must wake the waiter.
/// 无在途数据时 wait 快速 OOM；加载后另一线程 DecRef 应唤醒等待者。
#[test]
fn test_wait_ingest_data_released() {
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let contents = vec![vec![kv(&[1], b"first"), kv(&[2], b"second")]];
    let (data_files, stat_files) = write_contents(store.as_ref(), &contents);
    let mut engine = NewExternalEngine(
        store,
        data_files,
        stat_files,
        vec![1],
        vec![3],
        vec![vec![1], vec![3]],
        vec![vec![1], vec![3]],
        1,
        123,
        2,
        2,
        true,
        4 * 1024 * 1024,
        OnDuplicateKey::Ignore,
        "/".to_owned(),
    )
    .expect("NewExternalEngine should succeed");

    let batches = engine
        .LoadIngestData(&CancellationToken::default())
        .expect("load should succeed");
    assert_eq!(1, batches.len());
    let data = batches[0].data.clone();
    data.IncRef();
    data.DecRef();
    engine
        .waitIngestDataReleased()
        .expect("an already-buffered release signal must allow one retry");
    match engine.waitIngestDataReleased() {
        Err(Error::OutOfMemory { .. }) => {}
        other => panic!("expected OutOfMemory after consuming the release signal, got {other:?}"),
    }

    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let (data_files, stat_files) = write_contents(store.as_ref(), &contents);
    let mut engine = NewExternalEngine(
        store,
        data_files,
        stat_files,
        vec![1],
        vec![3],
        vec![vec![1], vec![3]],
        vec![vec![1], vec![3]],
        1,
        123,
        2,
        2,
        true,
        4 * 1024 * 1024,
        OnDuplicateKey::Ignore,
        "/".to_owned(),
    )
    .expect("NewExternalEngine should succeed");
    let batches = engine
        .LoadIngestData(&CancellationToken::default())
        .expect("load should succeed");
    let data = batches[0].data.clone();
    data.IncRef();

    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let released_for_thread = Arc::clone(&released);
    let data_for_thread = data.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        released_for_thread.store(true, Ordering::SeqCst);
        data_for_thread.DecRef();
    });

    engine
        .waitIngestDataReleased()
        .expect("waitIngestDataReleased should return once the batch is released");
    assert!(released.load(Ordering::SeqCst));
    handle.join().unwrap();
    engine.Close().unwrap();
}

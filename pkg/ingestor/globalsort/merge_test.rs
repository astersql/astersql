// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 全局排序（global sort）合并算子的单元测试。
//
// 覆盖 `splitDataFiles` 分片、`MergeOperator` 成功/取消/非法并发/重复键拒绝，
// 以及 `merge_overlapping_files_internal` 的去重统计与多文件归并后的有序输出。
// 全局排序：导入/加索引等场景把编码后的 KV（键值对）按键排序后再写入存储。
//
// 自 Go `merge_test.go` 移植；无 failpoint 时直接走生产入口的真实失败路径。

// Ported from pkg/ingestor/globalsort/merge_test.go. Go's `TestMergeOperator`
// drives `mergeOverlappingFilesInternal` failure paths through failpoints;
// this crate has no failpoint injection, so the adapted test instead exercises
// this port's own real failure paths (cancellation, disallowed concurrency,
// and duplicate-key rejection) directly through `MergeOverlappingFiles`.
// `TestMergeOverlappingFilesInternal`/`TestOnefileWriterManyRows` replace the
// `simplesst` writer with `encode_kvs`/`MemoryStorage`, which is this port's
// real on-disk KV representation, and verify the same duplicate-count and
// sorted-output invariants.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::merge::{
    MaxMergingFilesPerThread, NewMergeCollector, NewMergeOperator, SubtaskSummary, splitDataFiles,
};
use crate::reader::CancellationToken;
use crate::{Error, KvPair, MemoryStorage, OnDuplicateKey, Storage, encode_kvs};

/// 序列化修改 `MaxMergingFilesPerThread` 的测试，避免多线程 runner 竞态。
///
/// `MaxMergingFilesPerThread` is a crate-wide static; serialize tests that
/// mutate it so they don't race with each other under `cargo test`'s default
/// multi-threaded runner (Go's package tests run sequentially by default).
static MAX_MERGING_FILES_LOCK: Mutex<()> = Mutex::new(());

// 验证按并发度把路径列表切成尽量均匀的批次，并尊重每线程最大文件数上限。
#[test]
fn test_split_data_files() {
    let _guard = MAX_MERGING_FILES_LOCK.lock().unwrap();
    let all_paths: Vec<String> = (0..110).map(|index| index.to_string()).collect();
    let slice = |range: std::ops::Range<usize>| all_paths[range].to_vec();

    // (路径列表, 并发度, 期望分片) 用例，覆盖少文件、均分与余数场景。
    let cases: Vec<(Vec<String>, usize, Vec<Vec<String>>)> = vec![
        (slice(0..1), 1, vec![slice(0..1)]),
        (slice(0..2), 1, vec![slice(0..2)]),
        (slice(0..2), 4, vec![slice(0..2)]),
        (slice(0..3), 4, vec![slice(0..3)]),
        (slice(0..4), 4, vec![slice(0..2), slice(2..4)]),
        (slice(0..5), 4, vec![slice(0..3), slice(3..5)]),
        (slice(0..6), 4, vec![slice(0..2), slice(2..4), slice(4..6)]),
        (slice(0..7), 4, vec![slice(0..3), slice(3..5), slice(5..7)]),
        (
            slice(0..15),
            4,
            vec![slice(0..4), slice(4..8), slice(8..12), slice(12..15)],
        ),
        (
            slice(0..83),
            4,
            vec![slice(0..21), slice(21..42), slice(42..63), slice(63..83)],
        ),
        (
            slice(0..100),
            4,
            vec![slice(0..25), slice(25..50), slice(50..75), slice(75..100)],
        ),
        (
            slice(0..100),
            8,
            vec![
                slice(0..13),
                slice(13..26),
                slice(26..39),
                slice(39..52),
                slice(52..64),
                slice(64..76),
                slice(76..88),
                slice(88..100),
            ],
        ),
    ];
    for (index, (paths, concurrency, expected)) in cases.into_iter().enumerate() {
        let result = splitDataFiles(&paths, concurrency);
        assert_eq!(expected, result, "case-{index}");
    }

    // 临时下调每线程最大合并文件数，验证超限时会额外拆批。
    let backup = MaxMergingFilesPerThread.load(Ordering::Relaxed);
    MaxMergingFilesPerThread.store(10, Ordering::Relaxed);
    let restore = || MaxMergingFilesPerThread.store(backup, Ordering::Relaxed);

    let check = |count: usize, concurrency: usize, expected: Vec<std::ops::Range<usize>>| {
        let result = splitDataFiles(&slice(0..count), concurrency);
        let expected: Vec<Vec<String>> = expected.into_iter().map(slice).collect();
        assert_eq!(expected, result);
    };
    check(
        91,
        8,
        vec![
            0..10,
            10..19,
            19..28,
            28..37,
            37..46,
            46..55,
            55..64,
            64..73,
            73..82,
            82..91,
        ],
    );
    check(
        99,
        8,
        vec![
            0..10,
            10..20,
            20..30,
            30..40,
            40..50,
            50..60,
            60..70,
            70..80,
            80..90,
            90..99,
        ],
    );
    check(
        101,
        8,
        vec![
            0..10,
            10..20,
            20..29,
            29..38,
            38..47,
            47..56,
            56..65,
            65..74,
            74..83,
            83..92,
            92..101,
        ],
    );
    restore();
}

// 两个已排序输入文件成功合并为一个输出。
#[test]
fn test_merge_operator_success() {
    let _guard = MAX_MERGING_FILES_LOCK.lock().unwrap();
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    for (index, pair) in [
        (b"a".to_vec(), b"1".to_vec()),
        (b"b".to_vec(), b"2".to_vec()),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .write(
                &format!("/in/{index}.data"),
                encode_kvs(&[KvPair {
                    key: pair.0,
                    value: pair.1,
                }]),
            )
            .unwrap();
        store
            .write(&format!("/in/{index}.stat"), Vec::new())
            .unwrap();
    }

    let op = NewMergeOperator(
        CancellationToken::default(),
        store,
        0,
        "/out",
        0,
        None,
        None,
        1,
        false,
        OnDuplicateKey::Ignore,
    )
    .expect("merge operator");

    let outputs = crate::merge::MergeOverlappingFiles(
        &["/in/0.data".to_owned(), "/in/1.data".to_owned()],
        &op,
    )
    .expect("merge should succeed");
    assert_eq!(1, outputs.len());
}

// 并发度为 0 时构造 MergeOperator 应返回 InvalidArgument。
#[test]
fn test_merge_operator_rejects_zero_concurrency() {
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let result = NewMergeOperator(
        CancellationToken::default(),
        store,
        0,
        "/out",
        0,
        None,
        None,
        0,
        false,
        OnDuplicateKey::Ignore,
    );
    match result {
        Err(Error::InvalidArgument(_)) => {}
        Err(other) => panic!("expected InvalidArgument, got {other:?}"),
        Ok(_) => panic!("expected InvalidArgument, got Ok"),
    }
}

// 取消令牌已触发时，合并应立即以 Cancelled 失败。
#[test]
fn test_merge_operator_cancelled() {
    let _guard = MAX_MERGING_FILES_LOCK.lock().unwrap();
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let token = CancellationToken::default();
    token.cancel();
    let op = NewMergeOperator(
        token,
        store,
        0,
        "/out",
        0,
        None,
        None,
        1,
        false,
        OnDuplicateKey::Ignore,
    )
    .expect("merge operator");
    let error = crate::merge::MergeOverlappingFiles(&["/in/0.data".to_owned()], &op)
        .expect_err("cancelled token must abort the merge");
    assert!(matches!(error, Error::Cancelled));
}

// OnDuplicateKey::Error 遇到同键多值时返回 DuplicateKey。
#[test]
fn test_merge_operator_duplicate_key_error() {
    let _guard = MAX_MERGING_FILES_LOCK.lock().unwrap();
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    store
        .write(
            "/in/0.data",
            encode_kvs(&[
                KvPair {
                    key: b"k".to_vec(),
                    value: b"1".to_vec(),
                },
                KvPair {
                    key: b"k".to_vec(),
                    value: b"2".to_vec(),
                },
            ]),
        )
        .unwrap();
    let op = NewMergeOperator(
        CancellationToken::default(),
        store,
        0,
        "/out",
        0,
        None,
        None,
        1,
        false,
        OnDuplicateKey::Error,
    )
    .expect("merge operator");
    let error = crate::merge::MergeOverlappingFiles(&["/in/0.data".to_owned()], &op)
        .expect_err("duplicate key must be rejected");
    assert!(matches!(error, Error::DuplicateKey { .. }));
}

// 构造含一个重复键的序列，校验 Ignore 保留重复项、Collector 行数与处理字节数。
#[test]
fn test_merge_overlapping_files_internal_ignore_and_collector() {
    let store = MemoryStorage::default();
    // kv_count keys, with one duplicate inserted at the midpoint, mirroring
    // Go's TestMergeOverlappingFilesInternal at a size that stays fast under
    // `cargo test`'s default (non-benchmark) profile.
    let kv_count = 2000;
    let mut kvs = Vec::with_capacity(kv_count);
    for i in 0..kv_count {
        let value = if i == kv_count / 2 { i - 1 } else { i };
        let key = vec![(value % 256) as u8, (value / 256) as u8];
        let val = key.clone();
        kvs.push(KvPair { key, value: val });
    }
    store.write("/in/0.data", encode_kvs(&kvs)).unwrap();

    let summary = Arc::new(SubtaskSummary::default());
    let collector = NewMergeCollector(Some(summary.clone()));

    let conflicts = Mutex::new(crate::ConflictInfo::default());
    let output = crate::merge::merge_overlapping_files_internal(
        &CancellationToken::default(),
        &["/in/0.data".to_owned()],
        &store,
        0,
        "/out",
        "0",
        0,
        None,
        Some(&collector),
        false,
        OnDuplicateKey::Ignore,
        1,
        &conflicts,
    )
    .expect("merge should succeed");

    assert_eq!("/out/0.data", output);
    let merged = crate::decode_kvs(&store.read(&output).unwrap(), 0).unwrap();
    assert_eq!(
        kv_count,
        merged.len(),
        "OnDuplicateKey::Ignore must preserve duplicate keys like Go OneFileWriter"
    );
    let expected_processed_size: i64 = merged.iter().map(|pair| pair.encoded_size() as i64).sum();
    assert_eq!(kv_count as i64, summary.row_count.load(Ordering::Relaxed));
    assert_eq!(
        expected_processed_size,
        summary.processed.load(Ordering::Relaxed)
    );
}

// 对照 Go OneFileWriter：Remove 删除整个重复组；Record 在 data 中保留前两条，
// 仅把第三条及之后计入冲突。
#[test]
fn test_merge_duplicate_modes_match_one_file_writer() {
    let store = MemoryStorage::default();
    let input = vec![
        KvPair {
            key: b"dup".to_vec(),
            value: b"1".to_vec(),
        },
        KvPair {
            key: b"dup".to_vec(),
            value: b"2".to_vec(),
        },
        KvPair {
            key: b"dup".to_vec(),
            value: b"3".to_vec(),
        },
        KvPair {
            key: b"unique".to_vec(),
            value: b"4".to_vec(),
        },
    ];
    store.write("/in/modes.data", encode_kvs(&input)).unwrap();

    let run = |mode, writer_id: &str| {
        let conflicts = Mutex::new(crate::ConflictInfo::default());
        let output = crate::merge::merge_overlapping_files_internal(
            &CancellationToken::default(),
            &["/in/modes.data".to_owned()],
            &store,
            0,
            "/out",
            writer_id,
            0,
            None,
            None,
            false,
            mode,
            1,
            &conflicts,
        )
        .expect("merge should succeed");
        let kvs = crate::decode_kvs(&store.read(&output).unwrap(), 0).unwrap();
        let info = conflicts.into_inner().unwrap();
        (kvs, info)
    };

    let (removed, removed_info) = run(OnDuplicateKey::Remove, "remove");
    assert_eq!(vec![input[3].clone()], removed);
    assert_eq!(crate::ConflictInfo::default(), removed_info);

    let (recorded, recorded_info) = run(OnDuplicateKey::Record, "record");
    assert_eq!(
        vec![input[0].clone(), input[1].clone(), input[3].clone()],
        recorded
    );
    assert_eq!(1, recorded_info.count);
    assert_eq!(vec!["/out/record.dup"], recorded_info.files);
    assert_eq!(
        vec![input[2].clone()],
        crate::decode_kvs(&store.read("/out/record.dup").unwrap(), 0).unwrap()
    );
}

// Collector is updated after every successful Go OneFileWriter.WriteRow call,
// before duplicate filtering changes which rows reach the data file.
#[test]
fn test_merge_collector_counts_rows_before_duplicate_filtering() {
    let store = MemoryStorage::default();
    let input = vec![
        KvPair {
            key: b"dup".to_vec(),
            value: b"1".to_vec(),
        },
        KvPair {
            key: b"dup".to_vec(),
            value: b"2".to_vec(),
        },
        KvPair {
            key: b"unique".to_vec(),
            value: b"3".to_vec(),
        },
    ];
    store
        .write("/in/collector.data", encode_kvs(&input))
        .unwrap();

    let summary = Arc::new(SubtaskSummary::default());
    let collector = NewMergeCollector(Some(summary.clone()));
    let conflicts = Mutex::new(crate::ConflictInfo::default());
    crate::merge::merge_overlapping_files_internal(
        &CancellationToken::default(),
        &["/in/collector.data".to_owned()],
        &store,
        0,
        "/out",
        "remove-collector",
        0,
        None,
        Some(&collector),
        false,
        OnDuplicateKey::Remove,
        1,
        &conflicts,
    )
    .expect("remove merge should succeed");

    assert_eq!(
        input.len() as i64,
        summary.row_count.load(Ordering::Relaxed)
    );
    assert_eq!(
        input
            .iter()
            .map(|pair| pair.encoded_size() as i64)
            .sum::<i64>(),
        summary.processed.load(Ordering::Relaxed)
    );

    let error_summary = Arc::new(SubtaskSummary::default());
    let error_collector = NewMergeCollector(Some(error_summary.clone()));
    let error = crate::merge::merge_overlapping_files_internal(
        &CancellationToken::default(),
        &["/in/collector.data".to_owned()],
        &store,
        0,
        "/out",
        "error-collector",
        0,
        None,
        Some(&error_collector),
        false,
        OnDuplicateKey::Error,
        1,
        &Mutex::new(crate::ConflictInfo::default()),
    )
    .expect_err("the second duplicate row should fail");
    assert!(matches!(error, Error::DuplicateKey { .. }));
    assert_eq!(1, error_summary.row_count.load(Ordering::Relaxed));
    assert_eq!(
        input[0].encoded_size() as i64,
        error_summary.processed.load(Ordering::Relaxed)
    );
}

// 偶奇分片的两路输入经内部合并后恢复全局有序且无丢失。
#[test]
fn test_merge_sorted_unique_data_round_trip() {
    let store = MemoryStorage::default();
    let mut kvs: Vec<KvPair> = (0_u32..500)
        .map(|value| {
            let key = value.to_be_bytes().to_vec();
            KvPair {
                key: key.clone(),
                value: key,
            }
        })
        .collect();
    // Shuffle deterministically (reverse order) before writing to two
    // "already produced" files, matching the merge step's job of restoring
    // global order across overlapping inputs.
    let (left, right): (Vec<_>, Vec<_>) =
        kvs.iter().cloned().partition(|pair| pair.key[3] % 2 == 0);
    store.write("/in/even.data", encode_kvs(&left)).unwrap();
    store.write("/in/odd.data", encode_kvs(&right)).unwrap();

    let conflicts = Mutex::new(crate::ConflictInfo::default());
    let output = crate::merge::merge_overlapping_files_internal(
        &CancellationToken::default(),
        &["/in/even.data".to_owned(), "/in/odd.data".to_owned()],
        &store,
        0,
        "/out",
        "merged",
        0,
        None,
        None,
        false,
        OnDuplicateKey::Ignore,
        1,
        &conflicts,
    )
    .expect("merge should succeed");

    let merged = crate::decode_kvs(&store.read(&output).unwrap(), 0).unwrap();
    kvs.sort_by(|left, right| left.key.cmp(&right.key));
    assert_eq!(kvs, merged);
}

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

// DiskSorter 精简单测。
//
// 覆盖排序去重/seek、取消后可重试、多 Writer 并发，以及文件名与压缩算法边界。

use crate::disk_sorter::{
    DiskSorterOptions, FileMetadata, KvStats, KvStatsBucket, build_compactions, make_filename,
    open_disk_sorter, parse_filename, pick_compaction_files, split_compaction_files,
};
use crate::external_sorter::{ExternalSorter, Iterator};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

/// 创建带随机后缀的临时测试目录。
fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("extsort-disk-sorter-unit-{name}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

/// 从 first 起收集全部 KV，并断言无错误。
fn collect(iter: &mut dyn Iterator) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    if iter.first() {
        while iter.valid() {
            out.push((iter.unsafe_key().to_vec(), iter.unsafe_value().to_vec()));
            iter.next();
        }
    }
    assert!(iter.error().is_none());
    out
}

#[test]
/// 验证排序去重、seek/last、排序后禁止写入，以及 reopen/cleanup。
fn disk_sorter_orders_deduplicates_seeks_and_persists() {
    let dir = temp_dir("roundtrip");
    let ctx = CancellationToken::new();
    let sorter = open_disk_sorter(
        &dir,
        DiskSorterOptions {
            writer_buffer_size: 7,
            compaction_threshold: 2,
            ..Default::default()
        },
    )
    .unwrap();

    let mut writer = sorter.new_writer(&ctx).unwrap();
    let mut key = b"c".to_vec();
    let mut value = b"3".to_vec();
    // 故意乱序写入；同 key 保留首次值。
    writer.put(&key, &value).unwrap();
    key[0] = b'x';
    value[0] = b'x';
    writer.put(b"a", b"1").unwrap();
    writer.put(b"b", b"2-first").unwrap();
    writer.put(b"b", b"2-duplicate").unwrap();
    writer.close().unwrap();

    // 排序前不可迭代；重复 sort 应幂等。
    assert!(sorter.new_iterator(&ctx).is_err());
    sorter.sort(&ctx).unwrap();
    sorter.sort(&ctx).unwrap();
    assert!(sorter.is_sorted());
    assert!(sorter.new_writer(&ctx).is_err());

    let mut iter = sorter.new_iterator(&ctx).unwrap();
    assert_eq!(
        collect(iter.as_mut()),
        vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"b".to_vec(), b"2-first".to_vec()),
            (b"c".to_vec(), b"3".to_vec()),
        ]
    );
    assert!(iter.seek(b"bb"));
    assert_eq!(iter.unsafe_key(), b"c");
    assert!(iter.last());
    assert_eq!(iter.unsafe_key(), b"c");
    iter.close().unwrap();
    sorter.close().unwrap();

    let reopened = open_disk_sorter(&dir, DiskSorterOptions::default()).unwrap();
    assert!(reopened.is_sorted());
    let mut iter = reopened.new_iterator(&ctx).unwrap();
    assert_eq!(collect(iter.as_mut()).len(), 3);
    reopened.close_and_cleanup().unwrap();
    assert!(!dir.exists());
}

#[test]
/// 取消的 sort 失败后可用新 token 重试成功。
fn cancelled_sort_is_retryable() {
    let dir = temp_dir("cancel");
    let sorter = open_disk_sorter(&dir, DiskSorterOptions::default()).unwrap();
    let ctx = CancellationToken::new();
    let mut writer = sorter.new_writer(&ctx).unwrap();
    writer.put(b"a", b"1").unwrap();
    writer.close().unwrap();

    // 已取消的 ctx 使 sort 失败，状态仍可再次排序。
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(sorter.sort(&cancelled).is_err());
    assert!(!sorter.is_sorted());
    sorter.sort(&ctx).unwrap();
    sorter.close_and_cleanup().unwrap();
}

#[test]
/// 多 Writer 并发写入后结果有序且无丢失。
fn multiple_writers_flush_concurrently() {
    let dir = temp_dir("parallel");
    let sorter = open_disk_sorter(
        &dir,
        DiskSorterOptions {
            writer_buffer_size: 16,
            ..Default::default()
        },
    )
    .unwrap();
    let ctx = CancellationToken::new();
    std::thread::scope(|scope| {
        for writer_id in 0..8_u32 {
            let sorter = sorter.clone();
            let ctx = ctx.clone();
            scope.spawn(move || {
                let mut writer = sorter.new_writer(&ctx).unwrap();
                for item_id in 0..50_u32 {
                    let key = (writer_id * 50 + item_id).to_be_bytes();
                    writer.put(&key, &key).unwrap();
                }
                writer.close().unwrap();
            });
        }
    });
    sorter.sort(&ctx).unwrap();
    let mut iter = sorter.new_iterator(&ctx).unwrap();
    let records = collect(iter.as_mut());
    assert_eq!(records.len(), 400);
    assert!(records.windows(2).all(|pair| pair[0].0 < pair[1].0));
    sorter.close_and_cleanup().unwrap();
}

/// 构造无统计信息的精简 FileMetadata。
fn meta(num: u64, start: &[u8], end: &[u8]) -> FileMetadata {
    FileMetadata {
        file_num: num,
        start_key: start.to_vec(),
        end_key: end.to_vec(),
        last_key: end[..end.len().saturating_sub(1)].to_vec(),
        kv_stats: KvStats::default(),
    }
}

#[test]
/// 文件名解析与 pick/split/build_compactions 边界对齐 Go。
fn filename_and_compaction_algorithms_match_go_boundaries() {
    let dir = PathBuf::from("/tmp");
    assert_eq!(make_filename(&dir, 1), PathBuf::from("/tmp/000001.sst"));
    assert_eq!(
        parse_filename(PathBuf::from("/tmp/000123.sst").as_path()),
        Some(123)
    );
    assert_eq!(
        parse_filename(PathBuf::from("/tmp/123.sst.tmp").as_path()),
        None
    );

    let files = vec![
        meta(1, b"a", b"d"),
        meta(2, b"b", b"e"),
        meta(3, b"c", b"f"),
    ];
    let picked = pick_compaction_files(&files, 3);
    assert_eq!(
        picked.iter().map(|f| f.file_num).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(pick_compaction_files(&files, 4).is_empty());

    let groups = split_compaction_files(
        vec![
            meta(4, b"x", b"z"),
            meta(1, b"a", b"d"),
            meta(2, b"b", b"e"),
            meta(3, b"c", b"f"),
        ],
        2,
    );
    assert_eq!(
        groups.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![2, 1, 1]
    );

    let stats_files = vec![FileMetadata {
        kv_stats: KvStats {
            histogram: vec![
                KvStatsBucket {
                    size: 4,
                    upper_bound: b"b".to_vec(),
                },
                KvStatsBucket {
                    size: 4,
                    upper_bound: b"d".to_vec(),
                },
                KvStatsBucket {
                    size: 4,
                    upper_bound: b"f".to_vec(),
                },
            ],
        },
        ..meta(9, b"a", b"g")
    }];
    let compactions = build_compactions(&stats_files, 4);
    assert_eq!(compactions.len(), 3);
    assert_eq!(compactions[0].start_key, b"a");
    assert_eq!(compactions[0].end_key, b"b");
    assert_eq!(compactions[2].end_key, b"g");
}

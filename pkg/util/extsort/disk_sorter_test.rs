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

// DiskSorter 与 SST/归并/压缩辅助的完整测试。
//
// 覆盖公共外排例程、断点续写 reopen、KvStatsCollector、读写池并发、
// MergingIter 以及压缩文件挑选/切分算法。

use crate::disk_sorter::{
    DISK_SORTER_SORTED_FILE, DiskSorterOptions, FileMetadata, KV_STATS_PROP_KEY, KvStats,
    KvStatsBucket, KvStatsCollector, MergingIter, SstIter, SstReaderPool, SstWriter,
    build_compactions, make_filename, open_disk_sorter, parse_filename, pick_compaction_files,
    split_compaction_files,
};
use crate::external_sorter::{ExternalSorter, Iterator};
use crate::external_sorter_test::{
    TestKeyValue, gen_random_kvs, run_common_parallel_test, run_common_test,
};
use rand::rngs::StdRng;
use rand::{SeedableRng, seq::SliceRandom};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

/// 创建带随机后缀的临时测试目录。
fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("extsort-disk-sorter-{name}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

/// 构造测试 KV。
fn kv(key: &[u8], value: &[u8]) -> TestKeyValue {
    TestKeyValue {
        key: key.to_vec(),
        value: value.to_vec(),
    }
}

/// 构造无 last_key/统计的 FileMetadata。
fn file_meta(file_num: u64, start_key: &[u8], end_key: &[u8]) -> FileMetadata {
    FileMetadata {
        file_num,
        start_key: start_key.to_vec(),
        end_key: end_key.to_vec(),
        last_key: Vec::new(),
        kv_stats: KvStats::default(),
    }
}

/// 构造直方图桶。
fn bucket(size: usize, upper_bound: &[u8]) -> KvStatsBucket {
    KvStatsBucket {
        size,
        upper_bound: upper_bound.to_vec(),
    }
}

/// 用 SstWriter 写出有序 SST 并返回回调元数据。
fn write_sst_file(dirname: &Path, file_num: u64, kvs: &[TestKeyValue]) -> FileMetadata {
    let captured = Arc::new(Mutex::new(None));
    let callback_slot = Arc::clone(&captured);
    let mut writer = SstWriter::new(
        dirname,
        file_num,
        8,
        Some(Box::new(move |meta| {
            *callback_slot.lock().unwrap() = Some(meta);
        })),
    )
    .unwrap();
    for kv in kvs {
        writer.set(&kv.key, &kv.value).unwrap();
    }
    writer.close().unwrap();
    let metadata = captured.lock().unwrap().clone().unwrap();
    metadata
}

#[test]
/// 跑公共单 Writer 外排例程。
fn test_disk_sorter_common() {
    let dir = temp_dir("common");
    let sorter = open_disk_sorter(
        &dir,
        DiskSorterOptions {
            writer_buffer_size: 32 * 1024,
            compaction_threshold: 4,
            max_compaction_depth: 4,
            ..Default::default()
        },
    )
    .unwrap();
    run_common_test(&sorter);
    sorter.close_and_cleanup().unwrap();
}

#[test]
/// 跑公共多 Writer 并行外排例程。
fn test_disk_sorter_common_parallel() {
    let dir = temp_dir("common-parallel");
    let sorter = open_disk_sorter(
        &dir,
        DiskSorterOptions {
            writer_buffer_size: 32 * 1024,
            compaction_threshold: 4,
            max_compaction_depth: 4,
            ..Default::default()
        },
    )
    .unwrap();
    run_common_parallel_test(&sorter);
    sorter.close_and_cleanup().unwrap();
}

#[test]
/// 写入一半关闭后 reopen 续写，排序后再次 reopen 校验。
fn test_disk_sorter_reopen() {
    let dir = temp_dir("reopen");
    let options = DiskSorterOptions {
        writer_buffer_size: 32 * 1024,
        compaction_threshold: 4,
        max_compaction_depth: 4,
        ..Default::default()
    };
    let ctx = CancellationToken::new();
    let mut sorter = open_disk_sorter(&dir, options.clone()).unwrap();
    let mut rng = StdRng::seed_from_u64(0);
    let mut kvs = gen_random_kvs(&mut rng, 2000, 256, 1024);

    // 第一阶段只写一半并关闭（不清理目录）。
    let mut writer = sorter.new_writer(&ctx).unwrap();
    for item in &kvs[..1000] {
        writer.put(&item.key, &item.value).unwrap();
    }
    writer.close().unwrap();
    sorter.close().unwrap();

    // reopen 后应仍为未排序，可继续写入。
    sorter = open_disk_sorter(&dir, options).unwrap();
    assert!(!sorter.is_sorted());
    let mut writer = sorter.new_writer(&ctx).unwrap();
    for item in &kvs[1000..] {
        writer.put(&item.key, &item.value).unwrap();
    }
    writer.close().unwrap();
    kvs.sort_by(|left, right| left.key.cmp(&right.key));
    sorter.sort(&ctx).unwrap();

    let verify = |sorter: &crate::disk_sorter::DiskSorter| {
        let mut iter = sorter.new_iterator(&ctx).unwrap();
        let mut count = 0;
        if iter.first() {
            while iter.valid() {
                assert_eq!(kvs[count].key, iter.unsafe_key());
                assert_eq!(kvs[count].value, iter.unsafe_value());
                count += 1;
                iter.next();
            }
        }
        assert_eq!(kvs.len(), count);
        iter.close().unwrap();
    };
    verify(&sorter);
    assert!(sorter.is_sorted());
    sorter.close().unwrap();

    sorter = open_disk_sorter(
        &dir,
        DiskSorterOptions {
            writer_buffer_size: 32 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(sorter.is_sorted());
    verify(&sorter);
    sorter.close_and_cleanup().unwrap();
}

#[test]
/// 未达到 Go 的压缩阈值时，Sort 只排序文件元数据，不重写已有 SST。
fn test_sort_preserves_files_below_compaction_threshold() {
    let dir = temp_dir("sort-without-compaction");
    let ctx = CancellationToken::new();
    let sorter = open_disk_sorter(
        &dir,
        DiskSorterOptions {
            writer_buffer_size: 2,
            compaction_threshold: 3,
            ..Default::default()
        },
    )
    .unwrap();
    let mut writer = sorter.new_writer(&ctx).unwrap();
    writer.put(b"a", b"1").unwrap();
    writer.put(b"z", b"2").unwrap();
    writer.close().unwrap();

    let sst_count = || {
        fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "sst"))
            .count()
    };
    assert_eq!(sst_count(), 2);
    sorter.sort(&ctx).unwrap();
    assert_eq!(sst_count(), 2);
    sorter.close_and_cleanup().unwrap();
}

#[test]
/// 不同 bucket_size 下直方图切分与 Go 用例一致。
fn test_kv_stats_collector() {
    let kvs = [
        kv(b"aa", b"11"),
        kv(b"bb", b"22"),
        kv(b"cc", b"33"),
        kv(b"dd", b"44"),
        kv(b"ee", b"55"),
    ];
    let cases = [
        (
            0,
            vec![
                bucket(4, b"aa"),
                bucket(4, b"bb"),
                bucket(4, b"cc"),
                bucket(4, b"dd"),
                bucket(4, b"ee"),
            ],
        ),
        (
            4,
            vec![
                bucket(4, b"aa"),
                bucket(4, b"bb"),
                bucket(4, b"cc"),
                bucket(4, b"dd"),
                bucket(4, b"ee"),
            ],
        ),
        (
            7,
            vec![bucket(8, b"bb"), bucket(8, b"dd"), bucket(4, b"ee")],
        ),
        (50, vec![bucket(20, b"ee")]),
    ];
    // 对每种桶大小断言序列化后的直方图。
    for (bucket_size, histogram) in cases {
        let mut collector = KvStatsCollector::new(bucket_size);
        for item in &kvs {
            collector.add(&item.key, &item.value);
        }
        let mut properties = HashMap::new();
        collector.finish(&mut properties).unwrap();
        assert_eq!(properties.len(), 1);
        let stats: KvStats = serde_json::from_str(&properties[KV_STATS_PROP_KEY]).unwrap();
        assert_eq!(stats, KvStats { histogram });
    }
}

#[test]
/// 编号到六位（或更长）文件名。
fn test_make_filename() {
    let cases = [
        (1, "000001.sst"),
        (123, "000123.sst"),
        (666666, "666666.sst"),
        (7777777, "7777777.sst"),
    ];
    for (file_num, expected) in cases {
        assert_eq!(
            make_filename(Path::new("/tmp"), file_num),
            Path::new("/tmp").join(expected)
        );
    }
}

#[test]
/// 合法 sst 名可解析，tmp/标记文件返回 None。
fn test_parse_filename() {
    let cases = [
        ("/tmp/1.sst", Some(1)),
        ("/tmp/123.sst", Some(123)),
        ("/tmp/000001.sst", Some(1)),
        ("/tmp/000123.sst", Some(123)),
        ("/tmp/666666.sst", Some(666666)),
        ("/tmp/7777777.sst", Some(7777777)),
        ("/tmp/123.sst.tmp", None),
        (DISK_SORTER_SORTED_FILE, None),
    ];
    for (filename, expected) in cases {
        assert_eq!(parse_filename(Path::new(filename)), expected);
    }
}

#[test]
/// 有序写出后元数据键范围与统计正确。
fn test_sst_writer() {
    let dir = temp_dir("writer");
    let items = [
        kv(b"aa", b"11"),
        kv(b"bb", b"22"),
        kv(b"cc", b"33"),
        kv(b"dd", b"44"),
        kv(b"ee", b"55"),
    ];
    let meta = write_sst_file(&dir, 13, &items);
    assert!(make_filename(&dir, 13).exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    assert_eq!(meta.file_num, 13);
    assert_eq!(meta.start_key, b"aa");
    assert_eq!(meta.end_key, b"ee\0");
    assert_eq!(meta.last_key, b"ee");
    assert_eq!(
        meta.kv_stats.histogram,
        vec![bucket(8, b"bb"), bucket(8, b"dd"), bucket(4, b"ee")]
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 空文件仍落盘，键范围为空半开区间。
fn test_sst_writer_empty() {
    let dir = temp_dir("writer-empty");
    let meta = write_sst_file(&dir, 13, &[]);
    assert!(make_filename(&dir, 13).exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    assert!(meta.start_key.is_empty());
    assert_eq!(meta.end_key, vec![0]);
    assert!(meta.last_key.is_empty());
    assert_eq!(meta.kv_stats, KvStats::default());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 乱序写入失败且不留下文件。
fn test_sst_writer_error() {
    let dir = temp_dir("writer-error");
    let mut writer = SstWriter::new(&dir, 13, 0, None).unwrap();
    // 非严格递增触发失败；close 清理临时文件。
    writer.set(b"bb", b"11").unwrap();
    assert!(writer.set(b"aa", b"22").is_err());
    assert!(writer.close().is_err());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 同文件 get 共享 Arc；unref 归零后不可再开迭代器。
fn test_sst_reader_pool() {
    let dir = temp_dir("reader-pool");
    write_sst_file(&dir, 1, &[]);
    let pool = SstReaderPool::new(&dir);
    let reader1 = pool.get(1).unwrap();
    let reader2 = pool.get(1).unwrap();
    assert!(Arc::ptr_eq(&reader1, &reader2));
    pool.unref(1).unwrap();
    reader1.new_iter().unwrap().close().unwrap();
    pool.unref(1).unwrap();
    assert!(reader1.new_iter().is_err());
    assert!(std::panic::catch_unwind(|| pool.unref(1)).is_err());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 多线程反复 get/unref 后池为空。
fn test_sst_reader_pool_parallel() {
    let dir = temp_dir("reader-pool-parallel");
    for file_num in 1..=3 {
        write_sst_file(&dir, file_num, &[]);
    }
    let pool = Arc::new(SstReaderPool::new(&dir));
    let mut handles = Vec::new();
    for index in 0..17 {
        let pool = Arc::clone(&pool);
        handles.push(std::thread::spawn(move || {
            let file_num = index % 3 + 1;
            for _ in 0..10_000 {
                pool.get(file_num).unwrap();
                pool.unref(file_num).unwrap();
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(pool.reader_count(), 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// SstIter 导航与关闭时归还 ReaderPool。
fn test_sst_iter() {
    let dir = temp_dir("sst-iter");
    let items = [
        kv(b"aa", b"11"),
        kv(b"bb", b"22"),
        kv(b"cc", b"33"),
        kv(b"dd", b"44"),
        kv(b"ee", b"55"),
    ];
    write_sst_file(&dir, 1, &items);
    let pool = Arc::new(SstReaderPool::new(&dir));
    let reader = pool.get(1).unwrap();
    let raw = reader.new_iter().unwrap();
    let close_pool = Arc::clone(&pool);
    let mut iter = SstIter::new(raw, Some(Box::new(move || close_pool.unref(1))));
    assert!(iter.seek(b"bc"));
    assert_eq!(iter.unsafe_key(), b"cc");
    assert_eq!(iter.unsafe_value(), b"33");
    assert!(iter.first());
    assert_eq!(iter.unsafe_key(), b"aa");
    assert_eq!(iter.unsafe_value(), b"11");
    assert!(iter.next());
    assert_eq!(iter.unsafe_key(), b"bb");
    assert_eq!(iter.unsafe_value(), b"22");
    assert!(iter.last());
    assert_eq!(iter.unsafe_key(), b"ee");
    assert_eq!(iter.unsafe_value(), b"55");
    assert!(!iter.next());
    assert!(!iter.valid());
    iter.close().unwrap();
    assert_eq!(pool.reader_count(), 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 多路归并去重、seek/last 与池引用清零。
fn test_merging_iter() {
    let dir = temp_dir("merging-iter");
    let definitions = [
        (
            1,
            vec![
                kv(b"a0", b"va0"),
                kv(b"a1", b"va1"),
                kv(b"e0", b"ve0"),
                kv(b"e1", b"ve1"),
            ],
        ),
        (
            2,
            vec![
                kv(b"b0", b"vb0"),
                kv(b"b1", b"vb1"),
                kv(b"d0", b"vd0"),
                kv(b"d1", b"vd1"),
            ],
        ),
        (
            3,
            vec![
                kv(b"c0", b"vc0"),
                kv(b"c1", b"vc1"),
                kv(b"g0", b"vg0"),
                kv(b"g1", b"vg1"),
            ],
        ),
        (
            4,
            vec![kv(b"f0", b"vf0"), kv(b"f1", b"vf1"), kv(b"h1", b"vh1")],
        ),
        (
            5,
            vec![kv(b"f0", b"vf0"), kv(b"f2", b"vf2"), kv(b"h0", b"vh0")],
        ),
        (
            6,
            vec![
                kv(b"i0", b"vi0"),
                kv(b"i1", b"vi1"),
                kv(b"j0", b"vj0"),
                kv(b"j1", b"vj1"),
            ],
        ),
    ];
    let mut files: Vec<_> = definitions
        .iter()
        .map(|(number, items)| write_sst_file(&dir, *number, items))
        .collect();
    // 文件需按 start_key 排序后交给 MergingIter。
    files.sort_by(|left, right| left.start_key.cmp(&right.start_key));
    let pool = Arc::new(SstReaderPool::new(&dir));
    let open_pool = Arc::clone(&pool);
    let mut iter = MergingIter::new(
        files,
        Box::new(move |file| {
            let reader = open_pool.get(file.file_num)?;
            let raw = reader.new_iter()?;
            let close_pool = Arc::clone(&open_pool);
            let file_num = file.file_num;
            Ok(Box::new(SstIter::new(
                raw,
                Some(Box::new(move || close_pool.unref(file_num))),
            )) as Box<dyn Iterator>)
        }),
    );
    let mut actual = Vec::new();
    if iter.first() {
        // Go 版本只打开可能覆盖当前最小 key 的文件，而不是一次打开全部 SST。
        assert_eq!(pool.reader_count(), 1);
        while iter.valid() {
            actual.push(kv(iter.unsafe_key(), iter.unsafe_value()));
            iter.next();
        }
    }
    assert!(iter.error().is_none());
    let expected = vec![
        kv(b"a0", b"va0"),
        kv(b"a1", b"va1"),
        kv(b"b0", b"vb0"),
        kv(b"b1", b"vb1"),
        kv(b"c0", b"vc0"),
        kv(b"c1", b"vc1"),
        kv(b"d0", b"vd0"),
        kv(b"d1", b"vd1"),
        kv(b"e0", b"ve0"),
        kv(b"e1", b"ve1"),
        kv(b"f0", b"vf0"),
        kv(b"f1", b"vf1"),
        kv(b"f2", b"vf2"),
        kv(b"g0", b"vg0"),
        kv(b"g1", b"vg1"),
        kv(b"h0", b"vh0"),
        kv(b"h1", b"vh1"),
        kv(b"i0", b"vi0"),
        kv(b"i1", b"vi1"),
        kv(b"j0", b"vj0"),
        kv(b"j1", b"vj1"),
    ];
    assert_eq!(actual, expected);
    assert!(iter.seek(&[]));
    assert_eq!(iter.unsafe_key(), b"a0");
    assert!(!iter.seek(b"k"));
    assert!(iter.error().is_none());
    let mut indexes: Vec<_> = (0..actual.len()).collect();
    indexes.shuffle(&mut StdRng::seed_from_u64(0));
    for index in indexes {
        assert!(iter.seek(&actual[index].key));
        assert_eq!(iter.unsafe_key(), actual[index].key);
        assert_eq!(iter.unsafe_value(), actual[index].value);
    }
    assert!(iter.last());
    assert_eq!(iter.unsafe_key(), b"j1");
    assert_eq!(iter.unsafe_value(), b"vj1");
    assert!(!iter.next());
    assert!(iter.error().is_none());
    assert!(iter.first());
    assert_eq!(iter.unsafe_key(), b"a0");
    iter.close().unwrap();
    assert_eq!(pool.reader_count(), 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
/// 扫描线重叠深度达阈值才选中文件。
fn test_pick_compaction_files() {
    let five = || {
        vec![
            file_meta(1, b"a", b"c"),
            file_meta(2, b"b", b"f"),
            file_meta(3, b"d", b"g"),
            file_meta(4, b"e", b"i"),
            file_meta(5, b"h", b"j"),
        ]
    };
    let cases = vec![
        (
            vec![
                file_meta(1, b"a", b"b"),
                file_meta(2, b"b", b"c"),
                file_meta(3, b"c", b"d"),
            ],
            2,
            vec![],
        ),
        (
            vec![
                file_meta(1, b"a", b"b"),
                file_meta(2, b"b", b"d"),
                file_meta(3, b"c", b"e"),
            ],
            2,
            vec![2, 3],
        ),
        (five(), 2, vec![1, 2, 3, 4, 5]),
        (five(), 3, vec![2, 3, 4]),
        (five(), 4, vec![]),
    ];
    for (files, threshold, expected) in cases {
        let mut actual: Vec<_> = pick_compaction_files(&files, threshold)
            .into_iter()
            .map(|file| file.file_num)
            .collect();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }
}

#[test]
/// 按重叠组与深度限制切分压缩批次。
fn test_split_compaction_files() {
    let cases = vec![
        (
            vec![
                file_meta(1, b"a", b"c"),
                file_meta(2, b"b", b"f"),
                file_meta(3, b"d", b"g"),
                file_meta(4, b"e", b"i"),
                file_meta(5, b"h", b"j"),
            ],
            5,
            vec![vec![1, 2, 3, 4, 5]],
        ),
        (
            vec![
                file_meta(1, b"a", b"c"),
                file_meta(2, b"b", b"f"),
                file_meta(3, b"d", b"g"),
                file_meta(4, b"e", b"i"),
                file_meta(5, b"h", b"j"),
            ],
            4,
            vec![vec![1, 2, 3], vec![4, 5]],
        ),
        (
            vec![
                file_meta(1, b"a", b"c"),
                file_meta(2, b"b", b"f"),
                file_meta(3, b"d", b"e"),
                file_meta(4, b"g", b"i"),
                file_meta(5, b"h", b"j"),
            ],
            3,
            vec![vec![1, 2, 3], vec![4, 5]],
        ),
    ];
    for (files, depth, expected) in cases {
        let actual: Vec<Vec<_>> = split_compaction_files(files, depth)
            .into_iter()
            .map(|group| group.into_iter().map(|file| file.file_num).collect())
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
/// 按直方图体积切分 Compaction 区间。
fn test_build_compactions() {
    let with_stats = |number, start: &[u8], end: &[u8], histogram| FileMetadata {
        kv_stats: KvStats { histogram },
        ..file_meta(number, start, end)
    };
    let cases = vec![
        (
            vec![file_meta(1, b"a", b"c"), file_meta(2, b"b", b"d")],
            20,
            vec![vec![1, 2]],
        ),
        (
            vec![with_stats(
                1,
                b"a",
                b"e\0",
                vec![
                    bucket(20, b"b"),
                    bucket(23, b"c"),
                    bucket(21, b"d"),
                    bucket(25, b"e"),
                ],
            )],
            20,
            vec![vec![1], vec![1], vec![1], vec![1]],
        ),
        (
            vec![with_stats(
                1,
                b"a",
                b"e\0",
                vec![
                    bucket(20, b"b"),
                    bucket(23, b"c"),
                    bucket(17, b"d"),
                    bucket(25, b"e"),
                ],
            )],
            50,
            vec![vec![1], vec![1]],
        ),
        (
            vec![
                with_stats(
                    1,
                    b"a",
                    b"e\0",
                    vec![
                        bucket(20, b"b"),
                        bucket(23, b"c"),
                        bucket(17, b"d"),
                        bucket(25, b"e"),
                    ],
                ),
                with_stats(
                    2,
                    b"c",
                    b"g\0",
                    vec![bucket(21, b"d"), bucket(22, b"f"), bucket(20, b"g")],
                ),
            ],
            50,
            vec![vec![1, 2], vec![1, 2]],
        ),
    ];
    for (files, size, expected) in cases {
        let actual: Vec<Vec<_>> = build_compactions(&files, size)
            .into_iter()
            .map(|compaction| {
                compaction
                    .overlap_files
                    .into_iter()
                    .map(|file| file.file_num)
                    .collect()
            })
            .collect();
        assert_eq!(actual, expected);
    }
}

#[derive(Debug)]
struct IteratorFailure(Arc<()>);
impl std::fmt::Display for IteratorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("iterator failure")
    }
}
impl std::error::Error for IteratorFailure {}

// Inject only iterator failures; the same OpenIter boundary is isolated by
// TestMergingIter in the Go tests.
struct FailingIter {
    fail_at: &'static str,
    identity: Arc<()>,
    close_failure: Option<Arc<()>>,
    error: Option<crate::external_sorter::Error>,
    valid: bool,
    closes: Arc<std::sync::atomic::AtomicUsize>,
}
impl FailingIter {
    fn step(&mut self, operation: &str) -> bool {
        self.valid = operation != self.fail_at;
        if !self.valid {
            self.error = Some(Box::new(IteratorFailure(self.identity.clone())));
        }
        self.valid
    }
}
impl Iterator for FailingIter {
    fn seek(&mut self, _: &[u8]) -> bool {
        self.step("seek")
    }
    fn first(&mut self) -> bool {
        self.step("first")
    }
    fn next(&mut self) -> bool {
        self.step("next")
    }
    fn last(&mut self) -> bool {
        self.step("last")
    }
    fn valid(&self) -> bool {
        self.valid
    }
    fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.error.as_deref()
    }
    fn take_error(&mut self) -> Option<crate::external_sorter::Error> {
        self.error.take()
    }
    fn unsafe_key(&self) -> &[u8] {
        b"a"
    }
    fn unsafe_value(&self) -> &[u8] {
        b"value"
    }
    fn close(&mut self) -> crate::external_sorter::Result<()> {
        self.closes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.close_failure.take() {
            Some(identity) => Err(Box::new(IteratorFailure(identity))),
            None => Ok(()),
        }
    }
}

#[test]
fn iterator_error_ownership_survives_merge_and_close() {
    for operation in ["first", "seek", "next", "last"] {
        let identity = Arc::new(());
        let closes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut iter = MergingIter::new(
            vec![file_meta(1, b"a", b"z")],
            Box::new({
                let identity = identity.clone();
                let closes = closes.clone();
                move |_| {
                    Ok(Box::new(SstIter::new(
                        Box::new(FailingIter {
                            fail_at: operation,
                            identity: identity.clone(),
                            close_failure: None,
                            error: None,
                            valid: false,
                            closes: closes.clone(),
                        }),
                        None,
                    )))
                }
            }),
        );
        let valid = match operation {
            "first" => iter.first(),
            "seek" => iter.seek(b"a"),
            "next" => {
                assert!(iter.first());
                iter.next()
            }
            "last" => iter.last(),
            _ => unreachable!(),
        };
        assert!(!valid);
        assert!(iter.error().is_some());
        let error = iter.take_error().expect("transfer the original error");
        assert!(iter.take_error().is_none(), "transfer consumes only once");
        iter.close().unwrap();
        let actual = error
            .downcast_ref::<IteratorFailure>()
            .unwrap_or_else(|| panic!("lost error type during {operation}: {error}"));
        assert!(Arc::ptr_eq(&identity, &actual.0));
        assert_eq!(closes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[test]
fn merging_iter_keeps_both_read_and_close_errors() {
    for operation in ["first", "seek", "last"] {
        let read_id = Arc::new(());
        let close_id = Arc::new(());
        let closes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut iter = MergingIter::new(
            vec![file_meta(1, b"a", b"z")],
            Box::new({
                let read_id = read_id.clone();
                let close_id = close_id.clone();
                let closes = closes.clone();
                move |_| {
                    Ok(Box::new(FailingIter {
                        fail_at: operation,
                        identity: read_id.clone(),
                        close_failure: Some(close_id.clone()),
                        error: None,
                        valid: false,
                        closes: closes.clone(),
                    }))
                }
            }),
        );
        assert!(!match operation {
            "first" => iter.first(),
            "seek" => iter.seek(b"a"),
            _ => iter.last(),
        });
        let error = iter.take_error().unwrap();
        // Go errors.Join keeps both failures; it formats one error per line.
        assert_eq!(
            error.to_string(),
            "iterator failure\niterator failure",
            "{operation}"
        );
        let joined = error
            .downcast_ref::<crate::external_sorter::JoinedError>()
            .unwrap();
        assert!(Arc::ptr_eq(
            &read_id,
            &joined.errors()[0]
                .downcast_ref::<IteratorFailure>()
                .unwrap()
                .0
        ));
        assert!(Arc::ptr_eq(
            &close_id,
            &joined.errors()[1]
                .downcast_ref::<IteratorFailure>()
                .unwrap()
                .0
        ));
        assert!(error.source().unwrap().is::<IteratorFailure>());
        iter.close().unwrap();
        assert_eq!(closes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[test]
fn merging_iter_next_keeps_first_error() {
    let first_id = Arc::new(());
    let second_id = Arc::new(());
    let closes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut iter = MergingIter::new(
        vec![file_meta(1, b"a", b"z"), file_meta(2, b"a", b"z")],
        Box::new({
            let first_id = first_id.clone();
            let closes = closes.clone();
            move |file| {
                Ok(Box::new(FailingIter {
                    fail_at: "next",
                    identity: if file.file_num == 1 {
                        first_id.clone()
                    } else {
                        second_id.clone()
                    },
                    close_failure: None,
                    error: None,
                    valid: false,
                    closes: closes.clone(),
                }))
            }
        }),
    );
    assert!(iter.first());
    assert!(!iter.next());
    let error = iter.take_error().unwrap();
    assert!(Arc::ptr_eq(
        &first_id,
        &error.downcast_ref::<IteratorFailure>().unwrap().0
    ));
    iter.close().unwrap();
    assert_eq!(closes.load(std::sync::atomic::Ordering::SeqCst), 2);
}

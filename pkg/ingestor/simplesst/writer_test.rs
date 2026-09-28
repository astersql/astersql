// Copyright 2026 AsterSQL.

// 多文件排序 Writer 的单元测试。
//
// 验证 flush 时按键排序写出、`DuplicateMode::Remove` 丢弃整组重复键，
// 以及 `DuplicateMode::Error` 在关闭时返回重复键错误。
// Writer：内存缓冲 KV，超限或关闭时排序并写入数据文件与统计文件。

#[test]
fn canonical_writer_flushes_sorted_files_and_enforces_duplicate_policy() {
    use crate::kv_reader::KVReader;
    use crate::writer::{DuplicateMode, WriterBuilder};
    use crate::{Error, MemoryStorage};

    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    // Remove：同键整组丢弃；小内存上限促使必要时提前 flush。
    builder
        .set_memory_size_limit(1024)
        .set_on_duplicate(DuplicateMode::Remove)
        .set_prop_keys_distance(2);
    let mut writer = builder.build(storage.clone(), "bulk", "w");
    // 乱序写入；重复键 a 整组被移除，最终保留 b、c。
    writer.write_row(b"c", b"3").unwrap();
    writer.write_row(b"a", b"1").unwrap();
    writer.write_row(b"a", b"2").unwrap();
    writer.write_row(b"b", b"4").unwrap();
    let summary = writer.close().unwrap();
    assert_eq!(summary.TotalCnt, 2);
    assert_eq!(
        (summary.Min.as_slice(), summary.Max.as_slice()),
        (&b"b"[..], &b"c"[..])
    );
    assert!(summary.KVFileCount >= 1);

    // 汇总中的 Filenames[0] 为数据文件路径，读回后排序比对内容。
    let mut rows = Vec::new();
    for files in &summary.MultipleFilesStats {
        for pair in &files.Filenames {
            let mut reader = KVReader::from_storage(&storage, &pair[0], 0, 2).unwrap();
            while let Ok(row) = reader.next_kv() {
                rows.push(row);
            }
        }
    }
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (b"b".to_vec(), b"4".to_vec()),
            (b"c".to_vec(), b"3".to_vec())
        ]
    );

    // Error 策略：同键第二次写入后在 close/flush 时返回 DuplicateKey。
    let mut error_builder = WriterBuilder::new();
    error_builder.set_on_duplicate(DuplicateMode::Error);
    let mut error_writer = error_builder.build(storage, "bulk", "error");
    error_writer.write_row(b"x", b"1").unwrap();
    error_writer.write_row(b"x", b"2").unwrap();
    assert!(matches!(
        error_writer.close(),
        Err(Error::DuplicateKey { .. })
    ));
}

#[test]
fn writer_encodes_keyspace_before_sorting_and_summarizing() {
    use crate::MemoryStorage;
    use crate::kv_reader::KVReader;
    use crate::writer::WriterBuilder;
    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    builder.set_key_prefix(vec![0x78, 0, 0, 0, 7]);
    let mut writer = builder.build(storage.clone(), "bulk", "keyspace");
    writer.write_row(b"b", b"2").unwrap();
    writer.write_row(b"a", b"1").unwrap();
    let summary = writer.close().unwrap();
    assert_eq!(summary.Min, vec![0x78, 0, 0, 0, 7, b'a']);
    assert_eq!(summary.Max, vec![0x78, 0, 0, 0, 7, b'b']);
    let data_file = &summary.MultipleFilesStats[0].Filenames[0][0];
    let mut reader = KVReader::from_storage(&storage, data_file, 0, 1).unwrap();
    assert_eq!(reader.next_kv().unwrap().0, vec![0x78, 0, 0, 0, 7, b'a']);
}

#[test]
fn writer_flushes_to_external_object_sink() {
    use crate::writer::{WriterBuilder, WriterSink};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    struct RecordingSink(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);
    impl WriterSink for RecordingSink {
        fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().insert(path.into(), data.into());
            Ok(())
        }
    }

    let files = Arc::new(Mutex::new(BTreeMap::new()));
    let sink = Arc::new(RecordingSink(files.clone()));
    let mut writer = WriterBuilder::new().build_with_sink(sink, "10/20", "data/worker");
    writer.write_row(b"b", b"2").unwrap();
    writer.write_row(b"a", b"1").unwrap();
    let summary = writer.close().unwrap();
    assert_eq!(summary.KVFileCount, 1);
    let names = &summary.MultipleFilesStats[0].Filenames[0];
    let saved = files.lock().unwrap();
    assert!(saved.contains_key(&names[0]));
    assert!(saved.contains_key(&names[1]));
}

fn read_writer_rows(
    storage: &crate::MemoryStorage,
    summary: &crate::writer::WriterSummary,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut rows = Vec::new();
    for stat in &summary.MultipleFilesStats {
        for pair in &stat.Filenames {
            let mut reader =
                crate::kv_reader::KVReader::from_storage(storage, &pair[0], 0, 7).unwrap();
            loop {
                match reader.next_kv() {
                    Ok(row) => rows.push(row),
                    Err(error) if error.is_eof() => break,
                    Err(error) => panic!("unexpected reader error: {error}"),
                }
            }
        }
    }
    rows
}

/// 对照 Go `TestWriterFlushMultiFileNames`。
#[test]
fn test_writer_flush_multi_file_names() {
    use crate::MemoryStorage;
    use crate::writer::WriterBuilder;

    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    builder
        .set_prop_keys_distance(2)
        .set_memory_size_limit(3 * (16 + 20));
    let mut writer = builder.build(storage, "test", "0");
    for index in 0..10_u8 {
        writer.write_row(&[index; 10], &[255 - index; 10]).unwrap();
    }
    // Go `WrittenBytes` only reports raw key/value bytes from successfully
    // flushed batches. Three batches have flushed; the final row is buffered.
    assert_eq!(writer.written_bytes(), 9 * 20);
    let summary = writer.close().unwrap();
    assert_eq!(writer.written_bytes(), 10 * 20);
    assert_eq!(summary.KVFileCount, 4);
    for (index, files) in summary
        .MultipleFilesStats
        .iter()
        .flat_map(|stat| &stat.Filenames)
        .enumerate()
    {
        let data_suffix = format!("/test/0/{index}");
        let stat_suffix = format!("/test/0_stat/{index}");
        assert!(files[0].ends_with(&data_suffix), "{}", files[0]);
        assert!(files[1].ends_with(&stat_suffix), "{}", files[1]);
    }
}

/// Go marks a writer closed before its final flush, even when that flush fails.
#[test]
fn close_failure_still_closes_writer() {
    use crate::writer::WriterBuilder;
    use crate::{Error, MemoryStorage};

    let storage = MemoryStorage::default();
    storage
        .fail_next_writes_containing("close-failure", 3)
        .unwrap();
    let mut writer = WriterBuilder::new().build(storage, "close-failure", "0");
    writer.write_row(b"key", b"value").unwrap();

    assert!(writer.close().is_err());
    assert_eq!(writer.close().unwrap_err(), Error::Closed);
}

/// 对照 Go `TestWriterDuplicateDetect`。
#[test]
fn test_writer_duplicate_detect() {
    use crate::MemoryStorage;
    use crate::writer::{DuplicateMode, WriterBuilder};

    let mut builder = WriterBuilder::new();
    builder.set_on_duplicate(DuplicateMode::Error);
    let mut writer = builder.build(MemoryStorage::default(), "duplicate", "0");
    for value in [0_u8, 1, 2, 2, 3] {
        writer.write_row(&[value], &[value]).unwrap();
    }
    assert!(writer.close().is_err());
}

/// 对照 Go `TestMultiFileStat`。
#[test]
fn test_multi_file_stat() {
    use crate::writer::MultipleFilesStat;

    let mut stat = MultipleFilesStat {
        Filenames: vec![
            ["3".into(), "5".into()],
            ["1".into(), "3".into()],
            ["2".into(), "4".into()],
        ],
        ..MultipleFilesStat::default()
    };
    stat.build(&[vec![3], vec![1], vec![2]], &[vec![5], vec![3], vec![4]])
        .unwrap();
    assert_eq!(stat.MinKey, vec![1]);
    assert_eq!(stat.MaxKey, vec![5]);
    assert_eq!(stat.MaxOverlappingNum, 3);
    assert_eq!(
        stat.Filenames,
        vec![
            [String::from("1"), String::from("3")],
            [String::from("2"), String::from("4")],
            [String::from("3"), String::from("5")]
        ]
    );
}

/// 对照 Go `TestMultiFileStatOverlap`。
#[test]
fn test_multi_file_stat_overlap() {
    use crate::writer::{GetMaxOverlappingTotal, MultipleFilesStat};

    let first = MultipleFilesStat {
        MinKey: vec![1],
        MaxKey: vec![100],
        MaxOverlappingNum: 100,
        ..MultipleFilesStat::default()
    };
    let second = MultipleFilesStat {
        MinKey: vec![5],
        MaxKey: vec![102],
        MaxOverlappingNum: 90,
        ..MultipleFilesStat::default()
    };
    let mut third = MultipleFilesStat {
        MinKey: vec![111],
        MaxKey: vec![200],
        MaxOverlappingNum: 200,
        ..MultipleFilesStat::default()
    };
    assert_eq!(
        GetMaxOverlappingTotal(&[first.clone(), second.clone(), third.clone()]),
        200
    );
    third.MaxOverlappingNum = 70;
    assert_eq!(
        GetMaxOverlappingTotal(&[first.clone(), second.clone(), third.clone()]),
        190
    );
    third.MinKey = vec![0];
    assert_eq!(GetMaxOverlappingTotal(&[first, second, third]), 260);
}

/// 对照 Go `TestWriterMultiFileStat`：分组、文件排序和组内重叠均正确。
#[test]
fn test_writer_multi_file_stat() {
    use std::sync::atomic::Ordering;

    use crate::MemoryStorage;
    use crate::writer::{MultiFileStatNum, WriterBuilder};

    let old = MultiFileStatNum.swap(3, Ordering::AcqRel);
    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    builder.set_memory_size_limit(52).set_prop_keys_distance(2);
    let mut writer = builder.build(storage.clone(), "multi", "0");
    let rows = [
        ("key01", "key02"),
        ("key03", "key04"),
        ("key05", "key06"),
        ("key11", "key13"),
        ("key12", "key15"),
        ("key14", "key16"),
        ("key20", "key22"),
        ("key21", "key23"),
        ("key22", "key24"),
    ];
    for (start, end) in rows {
        writer.write_row(start.as_bytes(), b"56789").unwrap();
        writer.write_row(end.as_bytes(), b"56789").unwrap();
    }
    let summary = writer.close().unwrap();
    MultiFileStatNum.store(old, Ordering::Release);
    assert_eq!(summary.KVFileCount, 9);
    assert_eq!(summary.MultipleFilesStats.len(), 3);
    assert_eq!(
        summary
            .MultipleFilesStats
            .iter()
            .map(|stat| stat.MaxOverlappingNum)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        (summary.Min.as_slice(), summary.Max.as_slice()),
        (&b"key01"[..], &b"key24"[..])
    );
    let mut output = read_writer_rows(&storage, &summary);
    output.sort();
    assert_eq!(output.len(), 18);
}

/// 对照 Go `TestWriterSort`；Go 原测试标记为性能测试并跳过。
#[test]
#[ignore = "performance-only parity test, matching the skipped Go test"]
fn test_writer_sort() {
    let mut rows = (0..100_000)
        .map(|index| format!("abcabc{index:08}").into_bytes())
        .rev()
        .collect::<Vec<_>>();
    let mut expected = rows.clone();
    rows.sort_unstable();
    expected.sort();
    assert_eq!(rows, expected);
}

/// 对照 Go `TestFlushKVsRetry`：统计对象首次失败后重试并留下可读属性。
#[test]
fn test_flush_kvs_retry() {
    use crate::MemoryStorage;
    use crate::stat_reader::StatsReader;
    use crate::writer::WriterBuilder;

    let storage = MemoryStorage::default();
    storage.fail_next_writes_containing("_stat", 1).unwrap();
    let mut builder = WriterBuilder::new();
    builder.set_prop_keys_distance(4).set_memory_size_limit(100);
    let mut writer = builder.build(storage.clone(), "retry", "0");
    writer.write_row(b"key1", b"val1").unwrap();
    writer.write_row(b"key3", b"val3").unwrap();
    writer.write_row(b"key2", b"val2").unwrap();
    let summary = writer.close().unwrap();
    assert_eq!(storage.write_attempts_containing("_stat").unwrap(), 2);
    let stat_path = &summary.MultipleFilesStats[0].Filenames[0][1];
    let mut reader = StatsReader::from_storage(&storage, stat_path, 7).unwrap();
    let mut last = Vec::new();
    while let Ok(property) = reader.next_prop() {
        assert!(last < property.FirstKey);
        last = property.FirstKey;
    }
}

/// 对照 Go `TestGetAdjustedIndexBlockSize`。
#[test]
fn test_get_adjusted_index_block_size() {
    use crate::writer::{DefaultBlockSize, GetAdjustedBlockSize};

    let mib = 1024 * 1024;
    assert_eq!(
        GetAdjustedBlockSize(1 * mib, DefaultBlockSize),
        1 * mib as usize
    );
    assert_eq!(
        GetAdjustedBlockSize(15 * mib, DefaultBlockSize),
        16 * mib as usize
    );
    assert_eq!(
        GetAdjustedBlockSize(16 * mib, DefaultBlockSize),
        16 * mib as usize
    );
    assert_eq!(
        GetAdjustedBlockSize(17 * mib, DefaultBlockSize),
        17 * mib as usize
    );
    assert_eq!(
        GetAdjustedBlockSize(166 * mib, DefaultBlockSize),
        16 * mib as usize
    );
}

/// 对照 Go `TestWriterOnDup`：Record 与 Remove 的汇总及冲突文件内容。
#[test]
fn test_writer_on_dup() {
    use crate::MemoryStorage;
    use crate::writer::{DuplicateMode, WriterBuilder};

    let storage = MemoryStorage::default();
    let mut record_builder = WriterBuilder::new();
    record_builder
        .set_on_duplicate(DuplicateMode::Record)
        .set_memory_size_limit(240);
    let mut record = record_builder.build(storage.clone(), "record", "0");
    for _ in 0..5 {
        record.write_row(b"1111", b"vvvv").unwrap();
    }
    let summary = record.close().unwrap();
    assert_eq!((summary.TotalCnt, summary.ConflictInfo.Count), (2, 3));
    let mut conflict_reader =
        crate::kv_reader::KVReader::from_storage(&storage, &summary.ConflictInfo.Files[0], 0, 5)
            .unwrap();
    let mut conflicts = 0;
    while conflict_reader.next_kv().is_ok() {
        conflicts += 1;
    }
    assert_eq!(conflicts, 3);

    let mut remove_builder = WriterBuilder::new();
    remove_builder.set_on_duplicate(DuplicateMode::Remove);
    let mut remove = remove_builder.build(storage, "remove", "0");
    for key in [b"a", b"a", b"b"] {
        remove.write_row(key, b"v").unwrap();
    }
    let summary = remove.close().unwrap();
    assert_eq!(
        (summary.TotalCnt, summary.Min, summary.Max),
        (1, b"b".to_vec(), b"b".to_vec())
    );
}

/// 对照 Go `TestGetAdjustedMergeSortOverlapThresholdAndMergeSortFileCountStep`。
#[test]
fn test_get_adjusted_merge_sort_thresholds() {
    use crate::writer::{GetAdjustedMergeSortFileCountStep, GetAdjustedMergeSortOverlapThreshold};

    for (concurrency, expected) in [
        (1, 250),
        (2, 500),
        (4, 1000),
        (6, 1500),
        (8, 2000),
        (16, 4000),
        (17, 4000),
        (32, 4000),
    ] {
        assert_eq!(GetAdjustedMergeSortOverlapThreshold(concurrency), expected);
        assert_eq!(
            GetAdjustedMergeSortFileCountStep(concurrency),
            expected as usize
        );
    }
}

/// 对照 Go `TestRandPartitionedPrefix`。
#[test]
fn test_rand_partitioned_prefix() {
    use crate::writer::{IsValidPartition, rand_partitioned_prefix};

    let mut state = 0x1234_5678_9abc_def0;
    for _ in 0..2560 {
        let path = rand_partitioned_prefix("write-prefix", &mut state);
        let (partition, suffix) = path.split_once('/').unwrap();
        assert!(IsValidPartition(partition.as_bytes()));
        assert_eq!(suffix, "write-prefix");
    }
    for invalid in ["aa", "pa", "p1111000a", "pa111000"] {
        assert!(!IsValidPartition(invalid.as_bytes()));
    }
    assert!(IsValidPartition(b"p00000000"));
    assert!(IsValidPartition(b"p11110000"));
}

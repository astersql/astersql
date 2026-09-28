// Copyright 2026 AsterSQL.

// OneFileWriter（单文件 SST 写入器）的单元测试。
//
// 覆盖 `DuplicateMode::Record`：同键保留前两条写入数据文件，多余行记入冲突信息；
// 关闭后通过 `KVReader` 读回主文件有序 KV。
// SST（Sorted String Table）：有序键值文件，用于批量导入/ingest。

#[test]
fn canonical_one_file_writer_records_late_duplicate_rows_and_summary() {
    use crate::MemoryStorage;
    use crate::kv_reader::KVReader;
    use crate::writer::{DuplicateMode, WriterBuilder};

    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    // Record：重复键保留前两条，其余写入冲突文件；属性按每 2 个 key 切分。
    builder
        .set_on_duplicate(DuplicateMode::Record)
        .set_prop_keys_distance(2);
    let mut writer = builder.build_one_file(storage.clone(), "ingest", "writer-1");
    writer.init_part_size(5 * 1024 * 1024).unwrap();
    // 同键 a 写三次、b 一次；第三条 a 应进入 ConflictInfo。
    writer.write_row(b"a", b"1").unwrap();
    writer.write_row(b"a", b"2").unwrap();
    writer.write_row(b"a", b"3").unwrap();
    writer.write_row(b"b", b"4").unwrap();
    let summary = writer.close().unwrap();
    // TotalCnt 统计主数据文件行数；ConflictInfo.Count 为冲突行数。
    assert_eq!(summary.TotalCnt, 3);
    assert_eq!(summary.ConflictInfo.Count, 1);
    assert_eq!(summary.MultipleFilesStats.len(), 1);

    // 从汇总中的数据文件路径读回 KV，确认主文件只含保留的三条。
    let data_path = &summary.MultipleFilesStats[0].Filenames[0][0];
    let mut reader = KVReader::from_storage(&storage, data_path, 0, 2).unwrap();
    let mut rows = Vec::new();
    while let Ok(row) = reader.next_kv() {
        rows.push(row);
    }
    assert_eq!(
        rows,
        vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"a".to_vec(), b"2".to_vec()),
            (b"b".to_vec(), b"4".to_vec()),
        ]
    );
}

fn read_all_rows(storage: &crate::MemoryStorage, path: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut reader = crate::kv_reader::KVReader::from_storage(storage, path, 0, 7).unwrap();
    let mut rows = Vec::new();
    loop {
        match reader.next_kv() {
            Ok(row) => rows.push(row),
            Err(error) if error.is_eof() => break,
            Err(error) => panic!("unexpected read error: {error}"),
        }
    }
    rows
}

/// 对照 Go `TestOnefileWriterBasic`。
#[test]
fn test_onefile_writer_basic() {
    use crate::MemoryStorage;
    use crate::writer::WriterBuilder;

    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    builder
        .set_prop_size_distance(100)
        .set_prop_keys_distance(2);
    let mut writer = builder.build_one_file(storage.clone(), "basic", "0");
    writer.init_part_size(5 * 1024 * 1024).unwrap();
    let expected = (0..100)
        .map(|index| {
            (
                format!("key-{index:03}").into_bytes(),
                format!("value-{index:03}").into_bytes(),
            )
        })
        .collect::<Vec<_>>();
    for (key, value) in &expected {
        writer.write_row(key, value).unwrap();
    }
    let summary = writer.close().unwrap();
    assert_eq!(summary.TotalCnt, 100);
    let filename_prefix = crate::writer::join_path("basic", "0");
    let mut random_state = crate::writer::get_hash(&filename_prefix);
    let expected_data_path = crate::writer::join_path(
        &crate::writer::rand_partitioned_prefix(&filename_prefix, &mut random_state),
        "one-file",
    );
    let expected_stat_path = crate::writer::join_path(
        &(crate::writer::rand_partitioned_prefix(&filename_prefix, &mut random_state)
            + crate::file::STAT_SUFFIX),
        "one-file",
    );
    assert_eq!(
        summary.MultipleFilesStats[0].Filenames[0],
        [expected_data_path, expected_stat_path]
    );
    let data_path = &summary.MultipleFilesStats[0].Filenames[0][0];
    assert_eq!(read_all_rows(&storage, data_path), expected);
}

/// 对照 Go `TestOnefileWriterStat`。
#[test]
fn test_onefile_writer_stat() {
    use crate::MemoryStorage;
    use crate::stat_reader::StatsReader;
    use crate::writer::WriterBuilder;

    for distance in [1_u64, 2, 3, 7, 10] {
        let storage = MemoryStorage::default();
        let mut builder = WriterBuilder::new();
        builder
            .set_prop_size_distance(u64::MAX)
            .set_prop_keys_distance(distance);
        let mut writer = builder.build_one_file(storage.clone(), "stat", &distance.to_string());
        for index in 0..25 {
            writer
                .write_row(
                    format!("key-{index:03}").as_bytes(),
                    format!("value-{index:03}").as_bytes(),
                )
                .unwrap();
        }
        let summary = writer.close().unwrap();
        let stat_path = &summary.MultipleFilesStats[0].Filenames[0][1];
        let mut reader = StatsReader::from_storage(&storage, stat_path, 5).unwrap();
        let mut key_count = 0;
        let mut property_count = 0;
        loop {
            match reader.next_prop() {
                Ok(property) => {
                    assert!(property.Keys <= distance);
                    key_count += property.Keys;
                    property_count += 1;
                }
                Err(error) if error.is_eof() => break,
                Err(error) => panic!("unexpected property error: {error}"),
            }
        }
        assert_eq!(key_count, 25);
        assert_eq!(property_count, 25_usize.div_ceil(distance as usize));
    }
}

/// 对照 Go `TestOnefilePropOffset`。
#[test]
fn test_onefile_prop_offset() {
    use crate::MemoryStorage;
    use crate::stat_reader::StatsReader;
    use crate::writer::WriterBuilder;

    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    builder
        .set_prop_size_distance(50)
        .set_prop_keys_distance(2)
        .set_memory_size_limit(200);
    let mut writer = builder.build_one_file(storage.clone(), "offset", "0");
    for index in 0..200 {
        writer
            .write_row(format!("key-{index:04}").as_bytes(), b"value")
            .unwrap();
    }
    let summary = writer.close().unwrap();
    let stat_path = &summary.MultipleFilesStats[0].Filenames[0][1];
    let mut reader = StatsReader::from_storage(&storage, stat_path, 11).unwrap();
    let mut last = 0;
    while let Ok(property) = reader.next_prop() {
        assert!(property.Offset >= last);
        last = property.Offset;
    }
}

/// 对照 Go `TestOnefileWriterDupError`。
#[test]
fn test_onefile_writer_dup_error() {
    use crate::MemoryStorage;
    use crate::writer::{DuplicateMode, WriterBuilder};

    let mut builder = WriterBuilder::new();
    builder.set_on_duplicate(DuplicateMode::Error);
    let mut writer = builder.build_one_file(MemoryStorage::default(), "dup-error", "0");
    writer.write_row(b"a", b"1").unwrap();
    assert!(writer.write_row(b"a", b"2").is_err());

    // Go increments the duplicate count before returning the write error. Closing then
    // discards that duplicate pivot instead of reporting the same error a second time.
    let summary = writer.close().unwrap();
    assert_eq!(summary.TotalCnt, 0);
    assert!(summary.MultipleFilesStats.is_empty());
}

/// 对照 Go `TestOneFileWriterOnDupRemove`。
#[test]
fn test_onefile_writer_on_dup_remove() {
    use crate::MemoryStorage;
    use crate::writer::{DuplicateMode, WriterBuilder};

    let storage = MemoryStorage::default();
    let mut builder = WriterBuilder::new();
    builder.set_on_duplicate(DuplicateMode::Remove);
    let mut writer = builder.build_one_file(storage.clone(), "dup-remove", "0");
    for value in [b"1", b"1", b"1", b"2", b"3", b"3"] {
        writer.write_row(value, b"v").unwrap();
    }
    let summary = writer.close().unwrap();
    assert_eq!(summary.TotalCnt, 1);
    assert_eq!(
        (summary.Min.as_slice(), summary.Max.as_slice()),
        (&b"2"[..], &b"2"[..])
    );
    assert_eq!(
        read_all_rows(&storage, &summary.MultipleFilesStats[0].Filenames[0][0]),
        vec![(b"2".to_vec(), b"v".to_vec())]
    );
}

/// Go marks a writer closed only after every object is written successfully.
#[test]
fn close_failure_does_not_make_writer_permanently_closed() {
    use crate::MemoryStorage;
    use crate::writer::WriterBuilder;

    let storage = MemoryStorage::default();
    storage.fail_next_writes_containing("_stat/", 1).unwrap();
    let mut writer = WriterBuilder::new().build_one_file(storage.clone(), "close-retry", "0");
    writer.write_row(b"a", b"1").unwrap();

    assert!(writer.close().is_err());
    let summary = writer.close().unwrap();
    assert_eq!(summary.TotalCnt, 1);
}

// Copyright 2026 AsterSQL.

// simplesst 工具函数单元测试。
//
// 覆盖区间端点最大重叠、已排序输入去重、以及按分区前缀枚举对象路径。
// 分区前缀形如 `p` + 8 位二进制，用于云对象 key 分散，降低限流。

#[test]
fn canonical_utilities_preserve_endpoint_duplicate_and_partition_semantics() {
    use crate::MemoryStorage;
    use crate::util::{
        Endpoint, EndpointTp, get_all_file_names, get_max_overlapping, remove_duplicates,
        remove_duplicates_more_than_two,
    };

    // [a,c] 权重 1 与 [b,d) 权重 2 在 b..c 重叠，最大累计权重为 3。
    let mut points = vec![
        Endpoint {
            Key: b"a".to_vec(),
            Tp: EndpointTp::InclusiveStart,
            Weight: 1,
        },
        Endpoint {
            Key: b"c".to_vec(),
            Tp: EndpointTp::InclusiveEnd,
            Weight: 1,
        },
        Endpoint {
            Key: b"b".to_vec(),
            Tp: EndpointTp::InclusiveStart,
            Weight: 2,
        },
        Endpoint {
            Key: b"d".to_vec(),
            Tp: EndpointTp::ExclusiveEnd,
            Weight: 2,
        },
    ];
    assert_eq!(get_max_overlapping(&mut points), 3);

    // keep=0：整组重复键全部移除；duplicates 计重复组元素总数。
    let mut rows = vec![(b"a".to_vec(), 1), (b"a".to_vec(), 2), (b"b".to_vec(), 3)];
    let (kept, removed, duplicates) = remove_duplicates(&mut rows, |row| row.0.as_slice(), true);
    assert_eq!(kept, vec![(b"b".to_vec(), 3)]);
    assert_eq!(removed.len(), 2);
    assert_eq!(duplicates, 2);

    // keep=2：同键保留前两条，第三条记入 removed。
    let mut triples = vec![(b"x".to_vec(), 1), (b"x".to_vec(), 2), (b"x".to_vec(), 3)];
    let (kept, removed, duplicates) =
        remove_duplicates_more_than_two(&mut triples, |row| row.0.as_slice());
    assert_eq!(kept.len(), 2);
    assert_eq!(removed.len(), 1);
    assert_eq!(duplicates, 3);

    // 合法路径：直接 `dir/...` 或 `pXXXXXXXX/dir/...`；非法分区前缀被跳过。
    let storage = MemoryStorage::default();
    storage.write("dir/a", vec![]).unwrap();
    storage.write("p00000001/dir/b", vec![]).unwrap();
    storage.write("invalid/dir/c", vec![]).unwrap();
    assert_eq!(
        get_all_file_names(&storage, "dir").unwrap(),
        vec!["dir/a".to_string(), "p00000001/dir/b".to_string()]
    );
}

/// 对照 Go `TestRemoveDuplicates`：覆盖无重复、首部、中部、尾部与混合重复组。
#[test]
fn test_remove_duplicates() {
    use crate::util::remove_duplicates;

    let cases = vec![
        (vec![], vec![], vec![]),
        (vec![1], vec![1], vec![]),
        (vec![1, 1, 2, 3], vec![2, 3], vec![1, 1]),
        (vec![1, 2, 2, 2, 3], vec![1, 3], vec![2, 2, 2]),
        (vec![1, 2, 3, 3], vec![1, 2], vec![3, 3]),
        (
            vec![1, 1, 2, 2, 2, 3, 4, 4, 5],
            vec![3, 5],
            vec![1, 1, 2, 2, 2, 4, 4],
        ),
    ];
    for (mut input, expected, removed) in cases {
        let (output, actual_removed, count) =
            remove_duplicates(&mut input, |value| std::slice::from_ref(value), true);
        assert_eq!(output, expected);
        assert_eq!(actual_removed, removed);
        assert_eq!(count, actual_removed.len());
    }
}

/// 对照 Go `TestRemoveDuplicatesMoreThan2`。
#[test]
fn test_remove_duplicates_more_than_two() {
    use crate::util::remove_duplicates_more_than_two;

    let cases = vec![
        (vec![], vec![], vec![], 0),
        (vec![1, 1], vec![1, 1], vec![], 2),
        (vec![1, 1, 1], vec![1, 1], vec![1], 3),
        (
            vec![1, 2, 2, 2, 3, 3, 3, 3, 4],
            vec![1, 2, 2, 3, 3, 4],
            vec![2, 3, 3],
            7,
        ),
    ];
    for (mut input, expected, removed, total) in cases {
        let (output, actual_removed, actual_total) =
            remove_duplicates_more_than_two(&mut input, |value| std::slice::from_ref(value));
        assert_eq!(output, expected);
        assert_eq!(actual_removed, removed);
        assert_eq!(actual_total, total);
    }
}

fn write_properties(
    storage: &crate::MemoryStorage,
    path: &str,
    properties: &[crate::codec::RangeProperty],
) {
    storage
        .write(path, crate::codec::encode_multi_props(properties).unwrap())
        .unwrap();
}

/// 对照 Go `TestGetReadRangeFromProps`。
#[test]
fn test_get_read_range_from_props() {
    use crate::MemoryStorage;
    use crate::codec::RangeProperty;
    use crate::util::get_read_range_from_props;

    let storage = MemoryStorage::default();
    write_properties(
        &storage,
        "props-1",
        &[
            RangeProperty {
                FirstKey: b"key1".to_vec(),
                Offset: 10,
                ..RangeProperty::default()
            },
            RangeProperty {
                FirstKey: b"key3".to_vec(),
                Offset: 30,
                ..RangeProperty::default()
            },
            RangeProperty {
                FirstKey: b"key5".to_vec(),
                Offset: 50,
                ..RangeProperty::default()
            },
        ],
    );
    write_properties(
        &storage,
        "props-2",
        &[
            RangeProperty {
                FirstKey: b"key2".to_vec(),
                Offset: 20,
                ..RangeProperty::default()
            },
            RangeProperty {
                FirstKey: b"key4".to_vec(),
                Offset: 40,
                ..RangeProperty::default()
            },
        ],
    );
    storage.write("props-empty", Vec::new()).unwrap();
    let paths = vec!["props-1".into(), "props-2".into(), "props-empty".into()];
    let keys = vec![
        b"key0".to_vec(),
        b"key1".to_vec(),
        b"key2.5".to_vec(),
        b"key3".to_vec(),
        b"key999".to_vec(),
    ];
    assert_eq!(
        get_read_range_from_props(&keys, &paths, &storage).unwrap(),
        vec![
            vec![0, 0, 0],
            vec![10, 0, 0],
            vec![10, 20, 0],
            vec![30, 20, 0],
            vec![50, 40, 0],
        ]
    );
}

/// 对照 Go `TestGetReadRangeFromPropsEmptyJobKeys`：不触发任何读取。
#[test]
fn test_get_read_range_from_props_empty_job_keys() {
    use crate::MemoryStorage;
    use crate::util::get_read_range_from_props;

    let storage = MemoryStorage::default();
    storage.set_read_delay(std::time::Duration::from_millis(10));
    assert!(
        get_read_range_from_props(&[], &["not-opened".into()], &storage)
            .unwrap()
            .is_empty()
    );
    assert_eq!(storage.max_concurrent_reads(), 0);
}

/// 对照 Go `TestGetReadRangeFromPropsLimitsParallelRead`。
#[test]
fn test_get_read_range_from_props_limits_parallel_read() {
    use crate::MemoryStorage;
    use crate::codec::RangeProperty;
    use crate::util::get_read_range_from_props_with_limit;

    let storage = MemoryStorage::default();
    let properties = [RangeProperty {
        FirstKey: b"key1".to_vec(),
        Offset: 10,
        ..RangeProperty::default()
    }];
    let paths = (0..5)
        .map(|index| format!("limit-{index}"))
        .collect::<Vec<_>>();
    for path in &paths {
        write_properties(&storage, path, &properties);
    }
    storage.reset_read_metrics();
    storage.set_read_delay(std::time::Duration::from_millis(20));
    let result =
        get_read_range_from_props_with_limit(&[b"key1".to_vec()], &paths, &storage, 2).unwrap();
    assert_eq!(result, vec![vec![10; 5]]);
    assert_eq!(storage.max_concurrent_reads(), 2);
}

/// Go 在首个属性已证明目标 key 位于文件开头之前时立即返回，不解析无关尾部。
#[test]
fn test_get_read_range_from_props_stops_after_all_keys_are_resolved() {
    use crate::MemoryStorage;
    use crate::codec::{RangeProperty, encode_multi_props};
    use crate::util::get_read_range_from_props;

    let storage = MemoryStorage::default();
    let mut data = encode_multi_props(&[RangeProperty {
        FirstKey: b"key2".to_vec(),
        Offset: 20,
        ..RangeProperty::default()
    }])
    .unwrap();
    data.extend_from_slice(&[0, 0, 0, 8, 1]);
    storage.write("props-with-corrupt-tail", data).unwrap();

    assert_eq!(
        get_read_range_from_props(
            &[b"key1".to_vec()],
            &["props-with-corrupt-tail".into()],
            &storage,
        )
        .unwrap(),
        vec![vec![0]],
    );
}

#[test]
fn batched_file_discovery_deduplicates_directories_and_keeps_writer_files_sorted() {
    let storage = crate::MemoryStorage::default();
    for dir in ["subtask", "subtask2", "kept"] {
        let mut builder = crate::writer::WriterBuilder::new();
        builder.set_memory_size_limit(100).set_prop_keys_distance(3);
        let mut writer = builder.build(storage.clone(), dir, "0");
        for key in 0..30u8 {
            writer.write_row(&[key], &[key]).unwrap();
        }
        writer.close().unwrap();
    }
    let mut expected = crate::util::GetAllFileNames(&storage, "subtask").unwrap();
    expected.extend(crate::util::GetAllFileNames(&storage, "subtask2").unwrap());
    expected.sort();
    assert!(!expected.is_empty());
    assert_eq!(
        crate::util::GetAllFileNamesInDirectories(&storage, &["subtask", "subtask2", "subtask"])
            .unwrap(),
        expected
    );
    assert!(
        crate::util::GetAllFileNamesInDirectories(&storage, &[])
            .unwrap()
            .is_empty()
    );
    assert!(
        !crate::util::GetAllFileNames(&storage, "kept")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn batched_file_discovery_scans_once_matches_segments_and_propagates_errors() {
    let calls = std::cell::Cell::new(0);
    let scan = || -> Result<Vec<String>, &'static str> {
        calls.set(calls.get() + 1);
        Ok([
            "43/meta.json",
            "42/plan/ingest/meta.json",
            "p00110000/42/data",
            "p11111111/43/stat",
            "420/data",
            "p0011000x/42/data",
            "p00110000/42",
            "42",
            "other/43/data",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect())
    };
    assert!(
        crate::util::GetAllFileNamesFromScan(&[], scan)
            .unwrap()
            .is_empty()
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        crate::util::GetAllFileNamesFromScan(&["42", "43", "42"], scan).unwrap(),
        vec![
            "42/plan/ingest/meta.json",
            "43/meta.json",
            "p00110000/42/data",
            "p11111111/43/stat"
        ]
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(
        crate::util::GetAllFileNamesFromScan(&["42"], || Err::<Vec<String>, _>("scan failed"))
            .unwrap_err(),
        "scan failed"
    );
}

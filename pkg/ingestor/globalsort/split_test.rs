// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `RangeSplitter` 与 `CalRangeSize` 的单元测试。
//
// 校验分组单调性、单组边界、文件数上界、精确键数阈值，以及常见 Region
// 大小配置下的 range 估算；大规模外部存储用例在无 URI 时跳过。

// Ported from pkg/ingestor/globalsort/split_test.go against the Rust
// RangeSplitter API (split.rs) and MockExternalEngine/mockOneMultiFileStat
// helpers (util.rs/testutil.rs). `Test3KFilesRangeSplitter` requires an
// external `--testing-storage-uri`, which is unset by default in Go CI too
// (openTestingStorage calls t.Skip when the flag is empty); the Rust port
// preserves that same skip behavior via `misc_bench_test::open_testing_storage`.

use crate::misc_bench_test::open_testing_storage;
use crate::split::{CalRangeSize, NewRangeSplitter};
use crate::testutil::mockOneMultiFileStat;
use crate::{FilePair, MemoryStorage, MockExternalEngine, MultipleFilesStat, Storage, encode_kvs};

/// 字节切片字典序比较，便于断言键单调性。
fn bytes_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.cmp(b)
}

// 全量扫描：组结束键与内部切分键均须严格递增，且落在 (prev_end, end) 内。
// test_general_properties corresponds to Go's TestGeneralProperties: it
// generates random key/value pairs and checks the monotonicity invariants
// SplitOneRangesGroup must uphold across a full pass.
#[test]
fn test_general_properties() {
    let mem_store = MemoryStorage::default();
    let kv_num = 500usize;
    let mut keys = Vec::with_capacity(kv_num);
    let mut values = Vec::with_capacity(kv_num);
    for i in 0..kv_num {
        keys.push(format!("key{i:06}").into_bytes());
        values.push(format!("val{i:06}").into_bytes());
    }

    let (data_files, stat_files) =
        MockExternalEngine(&mem_store, &keys, &values).expect("mock engine should be created");
    let multi_file_stat = mockOneMultiFileStat(&data_files, &stat_files).unwrap();
    let mut splitter = NewRangeSplitter(&multi_file_stat, &mem_store, 1000, 30, 1000, 1, 35, 1000)
        .expect("NewRangeSplitter should succeed");

    let mut last_end_key: Option<Vec<u8>> = None;
    loop {
        let group = splitter
            .SplitOneRangesGroup()
            .expect("SplitOneRangesGroup should succeed");
        let end_key = (!group.end_key_of_group.is_empty()).then(|| group.end_key_of_group.clone());

        if let (Some(end_key), Some(last)) = (&end_key, &last_end_key) {
            assert_eq!(std::cmp::Ordering::Greater, bytes_cmp(end_key, last));
        }
        assert_eq!(group.data_files.len(), group.stat_files.len());
        assert!(!group.data_files.is_empty());

        for keys_to_check in [
            &group.interior_range_job_keys,
            &group.interior_region_split_keys,
        ] {
            if !keys_to_check.is_empty() {
                for i in 1..keys_to_check.len() {
                    assert_eq!(
                        std::cmp::Ordering::Greater,
                        bytes_cmp(&keys_to_check[i], &keys_to_check[i - 1])
                    );
                }
                let lower = last_end_key.as_deref().unwrap_or(&[]);
                assert_eq!(
                    std::cmp::Ordering::Greater,
                    bytes_cmp(&keys_to_check[0], lower)
                );
                if let Some(end_key) = &end_key {
                    assert_eq!(
                        std::cmp::Ordering::Less,
                        bytes_cmp(keys_to_check.last().unwrap(), end_key)
                    );
                }
            }
        }

        let exhausted = end_key.is_none();
        last_end_key = end_key;
        if exhausted {
            break;
        }
    }
    splitter.Close().expect("splitter should close");
}

/// 写入单个 data/stat 文件对并包装为 `MultipleFilesStat`。
fn build_multi_file_stat(
    store: &MemoryStorage,
    prefix: &str,
    kvs: &[(Vec<u8>, Vec<u8>)],
) -> MultipleFilesStat {
    let data_file = format!("{prefix}.data");
    let stat_file = format!("{prefix}.stat");
    let pairs: Vec<_> = kvs
        .iter()
        .map(|(key, value)| crate::KvPair {
            key: key.clone(),
            value: value.clone(),
        })
        .collect();
    store.write(&data_file, encode_kvs(&pairs)).unwrap();
    store.write(&stat_file, Vec::new()).unwrap();
    MultipleFilesStat {
        filenames: vec![FilePair {
            data_file,
            stat_file,
            properties: Vec::new(),
        }],
    }
}

fn build_file_pair(store: &MemoryStorage, prefix: &str, keys: &[&[u8]]) -> FilePair {
    let data_file = format!("{prefix}.data");
    let stat_file = format!("{prefix}.stat");
    let pairs: Vec<_> = keys
        .iter()
        .map(|key| crate::KvPair {
            key: key.to_vec(),
            value: key.to_vec(),
        })
        .collect();
    store.write(&data_file, encode_kvs(&pairs)).unwrap();
    store.write(&stat_file, Vec::new()).unwrap();
    FilePair {
        data_file,
        stat_file,
        properties: Vec::new(),
    }
}

// 单文件：不同 rangeJobKeyCnt 影响内部 job 键，但不会产生组结束键。
// test_only_one_group corresponds to Go's TestOnlyOneGroup: with a single
// data/stat file pair, different rangeJobKeyCnt thresholds change whether an
// interior range job key is recorded, but the group is never split (endKey
// stays empty).
#[test]
fn test_only_one_group() {
    let mem_store = MemoryStorage::default();
    let stat = build_multi_file_stat(
        &mem_store,
        "/mock-test/5",
        &[(vec![1], vec![1]), (vec![2], vec![2])],
    );
    let multi_file_stat = vec![stat];

    let mut splitter = NewRangeSplitter(
        &multi_file_stat,
        &mem_store,
        1000,
        30,
        1000,
        10,
        i64::MAX,
        i64::MAX,
    )
    .expect("first splitter should be created");
    let group = splitter
        .SplitOneRangesGroup()
        .expect("first split should succeed");
    assert!(group.end_key_of_group.is_empty());
    assert_eq!(1, group.data_files.len());
    assert_eq!(1, group.stat_files.len());
    assert!(group.interior_range_job_keys.is_empty());
    assert!(group.interior_region_split_keys.is_empty());
    splitter
        .Close()
        .expect("first splitter close should succeed");

    let mut splitter = NewRangeSplitter(
        &multi_file_stat,
        &mem_store,
        1000,
        30,
        1000,
        1,
        i64::MAX,
        i64::MAX,
    )
    .expect("second splitter should be created");
    let group = splitter
        .SplitOneRangesGroup()
        .expect("second split should succeed");
    assert!(group.end_key_of_group.is_empty());
    assert_eq!(1, group.data_files.len());
    assert_eq!(1, group.stat_files.len());
    assert_eq!(vec![vec![2u8]], group.interior_range_job_keys);
    assert!(group.interior_region_split_keys.is_empty());
    splitter
        .Close()
        .expect("second splitter close should succeed");
}

// 有序多文件：每组文件数不超过由 rangesGroupKeys 推出的上界。
// test_sorted_data corresponds to Go's TestSortedData: it checks each
// returned group never exceeds the file-count upper bound implied by
// rangesGroupKeys and the average KV count per file.
#[test]
fn test_sorted_data() {
    let mem_store = MemoryStorage::default();
    let kv_num = 100usize;
    let keys: Vec<Vec<u8>> = (0..kv_num)
        .map(|i| format!("key{i:03}").into_bytes())
        .collect();
    let values: Vec<Vec<u8>> = (0..kv_num)
        .map(|i| format!("val{i:03}").into_bytes())
        .collect();

    let (data_files, stat_files) =
        MockExternalEngine(&mem_store, &keys, &values).expect("mock engine should be created");
    assert!(
        data_files.len() > 1,
        "expected multiple files, matching Go's chunked stat layout"
    );
    let avg_kv_per_file = (kv_num as f64 / data_files.len() as f64).ceil();
    let ranges_group_kv = 30i64;
    let group_file_num_upper_bound =
        (((ranges_group_kv - 1) as f64) / avg_kv_per_file).ceil() as usize + 1;

    let multi_file_stat = mockOneMultiFileStat(&data_files, &stat_files).unwrap();
    let mut splitter = NewRangeSplitter(
        &multi_file_stat,
        &mem_store,
        1000,
        ranges_group_kv,
        1000,
        10,
        i64::MAX,
        i64::MAX,
    )
    .expect("splitter should be created");
    loop {
        let group = splitter
            .SplitOneRangesGroup()
            .expect("split should succeed");
        assert!(group.data_files.len() <= group_file_num_upper_bound);
        assert!(group.stat_files.len() <= group_file_num_upper_bound);
        if group.end_key_of_group.is_empty() {
            break;
        }
    }
    splitter.Close().expect("splitter close should succeed");
}

// 对照 Go TestRangeSplitterStrictCase：文件最后一个 property 被越过后，
// 该文件必须从下一 ranges group 的 active 文件集合中移除。
#[test]
fn test_range_splitter_strict_case() {
    let store = MemoryStorage::default();
    let first_two_writers = MultipleFilesStat {
        filenames: vec![
            build_file_pair(&store, "/mock-test/1/0", &[b"key01", b"key11"]),
            build_file_pair(&store, "/mock-test/1/1", &[b"key21"]),
            build_file_pair(&store, "/mock-test/2/0", &[b"key02", b"key12"]),
            build_file_pair(&store, "/mock-test/2/1", &[b"key22"]),
        ],
    };
    let third_writer = MultipleFilesStat {
        filenames: vec![
            build_file_pair(&store, "/mock-test/3/0", &[b"key03", b"key13"]),
            build_file_pair(&store, "/mock-test/3/1", &[b"key23"]),
        ],
    };
    let mut splitter = NewRangeSplitter(
        &[first_two_writers, third_writer],
        &store,
        1000,
        2,
        1000,
        1,
        1000,
        1,
    )
    .expect("splitter should be created");

    let expected = [
        (
            b"key03".as_slice(),
            vec!["/mock-test/1/0.data", "/mock-test/2/0.data"],
            vec![b"key02".to_vec()],
        ),
        (
            b"key12".as_slice(),
            vec![
                "/mock-test/1/0.data",
                "/mock-test/2/0.data",
                "/mock-test/3/0.data",
            ],
            vec![b"key11".to_vec()],
        ),
        (
            b"key21".as_slice(),
            vec!["/mock-test/2/0.data", "/mock-test/3/0.data"],
            vec![b"key13".to_vec()],
        ),
        (
            b"key23".as_slice(),
            vec!["/mock-test/1/1.data", "/mock-test/2/1.data"],
            vec![b"key22".to_vec()],
        ),
    ];

    for (end_key, data_files, split_keys) in expected {
        let group = splitter
            .SplitOneRangesGroup()
            .expect("split should succeed");
        assert_eq!(end_key, group.end_key_of_group);
        assert_eq!(
            data_files,
            group
                .data_files
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(split_keys, group.interior_range_job_keys);
        assert_eq!(split_keys, group.interior_region_split_keys);
    }

    let last = splitter
        .SplitOneRangesGroup()
        .expect("last split should succeed");
    assert!(last.end_key_of_group.is_empty());
    assert_eq!(vec!["/mock-test/3/1.data"], last.data_files);
    assert!(last.interior_range_job_keys.is_empty());
    assert!(last.interior_region_split_keys.is_empty());

    let drained = splitter
        .SplitOneRangesGroup()
        .expect("drained split should succeed");
    assert!(drained.data_files.is_empty());
    assert!(drained.stat_files.is_empty());
    splitter.Close().expect("splitter should close");
}

// When the previous property exhausts its file, the file must be retired even
// if the next globally ordered property is not the last property of its file.
#[test]
fn test_exhaustion_follows_previous_property() {
    let store = MemoryStorage::default();
    let mut splitter = NewRangeSplitter(
        &[
            MultipleFilesStat {
                filenames: vec![build_file_pair(&store, "a", &[b"a", b"c"])],
            },
            MultipleFilesStat {
                filenames: vec![build_file_pair(&store, "b", &[b"b", b"d", b"e"])],
            },
        ],
        &store,
        1000,
        2,
        1000,
        i64::MAX,
        1000,
        i64::MAX,
    )
    .expect("splitter should be created");

    let first = splitter.SplitOneRangesGroup().unwrap();
    assert_eq!(b"c", first.end_key_of_group.as_slice());
    assert_eq!(vec!["a.data", "b.data"], first.data_files);

    let second = splitter.SplitOneRangesGroup().unwrap();
    assert_eq!(b"e", second.end_key_of_group.as_slice());
    assert_eq!(vec!["a.data", "b.data"], second.data_files);

    let last = splitter.SplitOneRangesGroup().unwrap();
    assert!(last.end_key_of_group.is_empty());
    assert_eq!(vec!["b.data"], last.data_files);
}

// 键数阈值恰好等于文件内键数时的边界行为。
// test_exactly_key_num corresponds to Go's TestExactlyKeyNum: it checks the
// boundary case where the key-count threshold exactly equals the number of
// keys in the (single) file, both for rangeJobKeyCnt and rangesGroupKeyCnt.
#[test]
fn test_exactly_key_num() {
    let mem_store = MemoryStorage::default();
    let kv_num = 3usize;
    let kvs: Vec<(Vec<u8>, Vec<u8>)> = (0..kv_num)
        .map(|i| {
            (
                format!("key{i:03}").into_bytes(),
                format!("value{i:03}").into_bytes(),
            )
        })
        .collect();
    let stat = build_multi_file_stat(&mem_store, "/mock-test/5", &kvs);
    let data_files = vec![stat.filenames[0].data_file.clone()];
    let stat_files = vec![stat.filenames[0].stat_file.clone()];
    let multi_file_stat = vec![stat];

    let mut splitter = NewRangeSplitter(&multi_file_stat, &mem_store, 1000, 100, 1000, 3, 1000, 3)
        .expect("maxRangeKeys splitter should be created");
    let group = splitter
        .SplitOneRangesGroup()
        .expect("maxRangeKeys split should succeed");
    assert!(group.end_key_of_group.is_empty());
    assert_eq!(data_files, group.data_files);
    assert_eq!(stat_files, group.stat_files);
    assert!(group.interior_range_job_keys.is_empty());
    assert!(group.interior_region_split_keys.is_empty());

    let mut splitter = NewRangeSplitter(&multi_file_stat, &mem_store, 1000, 3, 1000, 1, 1000, 2)
        .expect("rangesGroupKeys splitter should be created");
    let group = splitter
        .SplitOneRangesGroup()
        .expect("rangesGroupKeys split should succeed");
    assert!(group.end_key_of_group.is_empty());
    assert_eq!(data_files, group.data_files);
    assert_eq!(stat_files, group.stat_files);
    assert_eq!(
        vec![b"key001".to_vec(), b"key002".to_vec()],
        group.interior_range_job_keys
    );
    assert_eq!(vec![b"key002".to_vec()], group.interior_region_split_keys);
}

// 依赖外部 testing-storage-uri；未配置则跳过（与 Go CI 默认一致）。
// test_3k_files_range_splitter corresponds to Go's Test3KFilesRangeSplitter.
// The Go test requires an externally provisioned `--testing-storage-uri`
// (openTestingStorage calls t.Skip otherwise, which is exactly what happens
// in default Go CI); the Rust port mirrors that skip rather than fabricating
// a 64GB in-memory workload.
#[test]
fn test_3k_files_range_splitter() {
    if open_testing_storage().is_none() {
        eprintln!("skip: testing-storage-uri is not set");
        return;
    }
    unreachable!("external storage backend is unavailable in this port");
}

// 枚举常见 Region 大小，核对 CalRangeSize 与隐含 SST 文件数关系。
// test_cal_range_size corresponds to Go's TestCalRangeSize: it enumerates
// common region size/key settings and checks CalRangeSize's relationship
// between range size, range keys, and implied SST file count.
#[test]
fn test_cal_range_size() {
    const MI_B: i64 = 1024 * 1024;
    const GI_B: i64 = 1024 * 1024 * 1024;
    let common_used_region_size_settings: [(i64, i64); 4] = [
        (96 * MI_B, 960_000),
        (256 * MI_B, 2_560_000),
        (512 * MI_B, 5_120_000),
        (GI_B, 10_240_000),
    ];
    let cases: [(f64, [(i64, i64, i64); 4]); 2] = [
        (
            1.7_f64 * GI_B as f64,
            [
                (2 * 96 * MI_B, 2 * 960_000, 1),
                (256 * MI_B, 2_560_000, 1),
                (256 * MI_B + 1, 2_560_000, 2),
                (256 * MI_B + 1, 2_560_000, 4),
            ],
        ),
        (
            3.5_f64 * GI_B as f64,
            [
                (5 * 96 * MI_B, 5 * 960_000, 1),
                (512 * MI_B, 5_120_000, 1),
                (512 * MI_B, 5_120_000, 1),
                (512 * MI_B + 1, 5_120_000, 2),
            ],
        ),
    ];

    for (mem_per_core, range_infos) in cases {
        for (j, (region_split_size, region_split_keys)) in
            common_used_region_size_settings.iter().enumerate()
        {
            let (range_size, range_keys) =
                CalRangeSize(mem_per_core as i64, *region_split_size, *region_split_keys);
            let (expected_range_size, expected_range_key, expected_file_num) = range_infos[j];
            assert_eq!(expected_range_size, range_size);
            assert_eq!(expected_range_key, range_keys);
            if expected_range_size >= *region_split_size {
                assert_eq!(1, expected_file_num);
                assert_eq!(0, range_size % *region_split_size);
            } else {
                assert_eq!(
                    expected_file_num,
                    ((*region_split_size as f64) / (range_size as f64)).ceil() as i64
                );
            }
        }
    }
}

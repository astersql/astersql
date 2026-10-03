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

// globalsort 工具函数单元测试。
//
// 覆盖键最小/最大比较、计划/子任务 meta 路径、SortedKVMeta 合并、
// 归并排序数据文件划分、前缀清理，以及内外层 JSON 元数据读写。
// SortedKVMeta：已排序 KV 文件集合的元信息；merge-sort：外部归并排序。

// Ported from pkg/ingestor/globalsort/util_test.go. Go's `simplesst` writer
// tests (TestGetAllFileNames, TestCleanUpFiles) are adapted to write raw
// files directly into `MemoryStorage`, since this crate has no SST writer;
// `CleanUpFiles`/`list_prefix`/`delete_files` are exercised exactly as
// production code, just without the writer's own file-naming step. Go's
// The reflection-based `marshalInternalFields`/`marshalExternalFields`
// helpers (TestMarshalFields) have no Rust reflection counterpart;
// `BaseExternalMeta` pushes internal/external field splitting to a
// caller-supplied `ExternalMetaCodec`, so `TestReadWriteJSON` is adapted to a
// hand-written codec that plays the same role as Go's reflection tags.

use crate::{
    BaseExternalMeta, BytesMax, BytesMin, CleanUpFiles, ConflictInfo, DivideMergeSortDataFiles,
    ExternalMetaCodec, FilePair, MultipleFilesStat, NewSortedKVMeta, PlanMetaPath,
    PreparedMetaPath, Storage, SubtaskMetaPath, WriterSummary,
};
use crate::{MemoryStorage, merge::splitDataFiles};

/// 校验 BytesMin / BytesMax 对字节切片的字典序比较。
#[test]
fn test_key_min_max() {
    assert_eq!(b"a", BytesMin(b"a", b"b"));
    assert_eq!(b"a", BytesMin(b"b", b"a"));
    assert_eq!(b"b", BytesMax(b"a", b"b"));
    assert_eq!(b"b", BytesMax(b"b", b"a"));
}

/// 校验计划、prepared、子任务 meta.json 路径拼装。
#[test]
fn test_external_meta_path() {
    assert_eq!(
        "1/plan/merge-sort/1/meta.json",
        PlanMetaPath(1, "merge-sort", 1)
    );
    assert_eq!("2/plan/ingest/3/meta.json", PlanMetaPath(2, "ingest", 3));
    assert_eq!("1/plan/prepared/meta.json", PreparedMetaPath(1));
    assert_eq!("2/plan/prepared/meta.json", PreparedMetaPath(2));
    assert_eq!("1/1/meta.json", SubtaskMetaPath(1, 1));
    assert_eq!("2/3/meta.json", SubtaskMetaPath(2, 3));
}

/// 构造测试用 WriterSummary（含键范围、体积、文件对与冲突信息）。
fn summary(
    min: &[u8],
    max: &[u8],
    total_size: u64,
    filenames: Vec<FilePair>,
    conflict: ConflictInfo,
) -> WriterSummary {
    WriterSummary {
        min: min.to_vec(),
        max: max.to_vec(),
        total_size,
        total_count: 0,
        multiple_files_stats: vec![MultipleFilesStat { filenames }],
        conflict_info: conflict,
    }
}

/// 构造 data/stat 文件对。
fn file_pair(data_file: &str, stat_file: &str) -> FilePair {
    FilePair {
        data_file: data_file.to_owned(),
        stat_file: stat_file.to_owned(),
        properties: Vec::new(),
    }
}

#[test]
fn test_summary_for_file_property_totals() {
    let kvs: Vec<_> = (0..5)
        .map(|i| crate::KvPair {
            key: vec![i],
            value: vec![i; i as usize + 1],
        })
        .collect();
    let summary =
        crate::util::summary_for_file("/out/0.data".to_owned(), "/out/0.stat".to_owned(), &kvs);
    let properties = &summary.multiple_files_stats[0].filenames[0].properties;
    assert_eq!(2, properties.len());
    assert_eq!(4, properties[0].keys);
    assert_eq!(
        kvs[..4]
            .iter()
            .map(|pair| pair.encoded_size() as u64)
            .sum::<u64>(),
        properties[0].size
    );
    assert_eq!(1, properties[1].keys);
    assert_eq!(kvs[4].encoded_size() as u64, properties[1].size);
}

/// 校验 NewSortedKVMeta、MergeSummary、Merge 对范围/体积/冲突的累积。
#[test]
fn test_sorted_kv_meta() {
    let summary0 = summary(
        b"a",
        b"b",
        123,
        vec![file_pair("f1", "stat1"), file_pair("f2", "stat2")],
        ConflictInfo {
            count: 1,
            files: vec!["dup0".to_owned()],
        },
    );
    let summary1 = summary(
        b"x",
        b"y",
        177,
        vec![file_pair("f3", "stat3"), file_pair("f4", "stat4")],
        ConflictInfo::default(),
    );

    let mut meta0 = NewSortedKVMeta(Some(&summary0));
    assert_eq!(b"a", meta0.StartKey.as_slice());
    // EndKey 为 max 后追加 0 字节，表示半开区间上界。
    assert_eq!(vec![b'b', 0], meta0.EndKey);
    assert_eq!(123, meta0.TotalKVSize);
    assert_eq!(summary0.multiple_files_stats, meta0.MultipleFilesStats);
    assert_eq!(
        ConflictInfo {
            count: 1,
            files: vec!["dup0".to_owned()],
        },
        meta0.ConflictInfo
    );

    let meta1 = NewSortedKVMeta(Some(&summary1));
    assert_eq!(b"x", meta1.StartKey.as_slice());
    assert_eq!(vec![b'y', 0], meta1.EndKey);
    assert_eq!(177, meta1.TotalKVSize);
    assert_eq!(ConflictInfo::default(), meta1.ConflictInfo);

    // 合并后取更广键范围、累加体积与文件统计、累加冲突。
    meta0.MergeSummary(Some(&summary1));
    assert_eq!(b"a", meta0.StartKey.as_slice());
    assert_eq!(vec![b'y', 0], meta0.EndKey);
    assert_eq!(300, meta0.TotalKVSize);
    let mut merged_stats = summary0.multiple_files_stats.clone();
    merged_stats.extend(summary1.multiple_files_stats.iter().cloned());
    assert_eq!(merged_stats, meta0.MultipleFilesStats);
    assert_eq!(
        ConflictInfo {
            count: 1,
            files: vec!["dup0".to_owned()],
        },
        meta0.ConflictInfo
    );

    let mut meta00 = NewSortedKVMeta(Some(&summary0));
    meta00.Merge(&meta1);
    assert_eq!(meta0, meta00);

    let extra = summary(
        b"xx",
        b"yy",
        0,
        Vec::new(),
        ConflictInfo {
            count: 2,
            files: vec!["dup1".to_owned()],
        },
    );
    meta0.MergeSummary(Some(&extra));
    assert_eq!(
        ConflictInfo {
            count: 3,
            files: vec!["dup0".to_owned(), "dup1".to_owned()],
        },
        meta0.ConflictInfo
    );
}

/// Go 的 uint64 累加按模 2^64 回绕；Rust 合并必须保持相同边界语义。
#[test]
fn test_sorted_kv_meta_merge_wraps_counters_like_go() {
    let mut meta = crate::SortedKVMeta {
        StartKey: b"a".to_vec(),
        EndKey: b"b".to_vec(),
        TotalKVSize: u64::MAX,
        TotalKVCnt: u64::MAX,
        ..Default::default()
    };
    meta.Merge(&crate::SortedKVMeta {
        StartKey: b"c".to_vec(),
        EndKey: b"d".to_vec(),
        TotalKVSize: 1,
        TotalKVCnt: 1,
        ..Default::default()
    });

    assert_eq!(0, meta.TotalKVSize);
    assert_eq!(0, meta.TotalKVCnt);
}

/// 校验 DivideMergeSortDataFiles 在典型文件数/节点数下的分组大小。
#[test]
fn test_divide_merge_sort_data_files_basic() {
    let cases = [
        (31_usize, 3_usize, vec![31_usize]),
        (64, 2, vec![32, 32]),
        (64, 3, vec![32, 32]),
        (127, 3, vec![43, 42, 42]),
        (128, 3, vec![43, 43, 42]),
        (4000, 6, vec![667, 667, 667, 667, 666, 666]),
        (4000, 7, vec![572, 572, 572, 571, 571, 571, 571]),
        (
            40000,
            7,
            vec![
                4000, 4000, 4000, 4000, 4000, 4000, 4000, 1715, 1715, 1714, 1714, 1714, 1714, 1714,
            ],
        ),
        (
            31000,
            7,
            vec![
                4000, 4000, 4000, 4000, 4000, 4000, 4000, 429, 429, 429, 429, 428, 428, 428,
            ],
        ),
        (
            28100,
            7,
            vec![4000, 4000, 4000, 4000, 4000, 4000, 4000, 34, 33, 33],
        ),
        (28031, 7, vec![4000, 4000, 4000, 4000, 4000, 4000, 4000, 31]),
    ];
    for (file_count, node_count, expected_sizes) in cases {
        let items = vec![String::new(); file_count];
        let result =
            DivideMergeSortDataFiles(&items, node_count, 16).expect("divide should succeed");
        let actual_sizes: Vec<usize> = result.iter().map(Vec::len).collect();
        assert_eq!(
            expected_sizes, actual_sizes,
            "fileCnt={file_count} nodeCnt={node_count}"
        );
    }
}

/// 大规模划分后子任务数与目标文件数须落在上限内（≤250 组、≤4000 目标文件）。
#[test]
fn test_divide_merge_sort_data_files_subtask_count() {
    const CONCURRENCY: usize = 16;
    for file_count in [3000_usize, 4000, 40000, 400000, 712345, 1000000] {
        for node_count in [1_usize, 3, 7, 16, 30, 60, 97] {
            let data_files = vec![String::new(); file_count];
            let data_files_group = DivideMergeSortDataFiles(&data_files, node_count, CONCURRENCY)
                .expect("divide should succeed");
            let total_target_file_count: usize = data_files_group
                .iter()
                .map(|group| splitDataFiles(group, CONCURRENCY).len())
                .sum();
            assert!(data_files_group.len() <= 250);
            assert!(total_target_file_count <= 4000);
        }
    }
}

/// 校验 CleanUpFiles 只删除指定前缀下文件，不影响其他前缀。
#[test]
fn test_clean_up_files() {
    let store = MemoryStorage::default();
    store.write("/subtask/0/0", vec![0]).expect("write");
    store.write("/subtask/0/1", vec![1]).expect("write");
    store.write("/subtask/0_stat/0", vec![2]).expect("write");
    store.write("/other/0", vec![3]).expect("write");

    let mut names = store.list_prefix("/subtask").expect("list");
    names.sort();
    assert_eq!(
        vec![
            "/subtask/0/0".to_owned(),
            "/subtask/0/1".to_owned(),
            "/subtask/0_stat/0".to_owned(),
        ],
        names
    );

    CleanUpFiles(&store, "/subtask").expect("clean up");
    assert!(store.list_prefix("/subtask").expect("list").is_empty());
    assert_eq!(
        vec!["/other/0".to_owned()],
        store.list_prefix("/other").expect("list")
    );
}

#[test]
fn cleanup_removes_random_partition_files_and_preserves_neighbor_tasks() {
    let store = MemoryStorage::default();
    let removed = [
        "30001/6/meta.json",
        "30001/plan/ingest/1/meta.json",
        "p00110000/30001/7/writer_stat/one-file",
        "p00000000/30001/7/writer/one-file",
        "/p11111111/30001/8/writer/one-file",
    ];
    let kept = [
        "300010/6/meta.json",
        "30002/6/meta.json",
        "p00000000/30002/7/writer/one-file",
        "p0000000x/30001/7/writer/one-file",
        "p0000000/30001/7/writer/one-file",
        "other/30001/7/writer/one-file",
        "p00000000/30001",
        "30001",
    ];
    for path in removed.iter().chain(&kept) {
        store.write(path, vec![1]).unwrap();
    }
    CleanUpFiles(&store, "30001").unwrap();
    for path in removed {
        assert!(store.read(path).is_err(), "task file survives: {path}");
    }
    for path in kept {
        assert_eq!(store.read(path).unwrap(), vec![1], "neighbor file: {path}");
    }
}

// Mirrors the shape of Go's `testStruct` in TestReadWriteJSON: `X` stays
// internal-only while `Y` is written to external storage. Go derives this
// split from `external:"true"` tags via reflection; this port expresses the
// same split explicitly through `ExternalMetaCodec`.
/// 手写 ExternalMetaCodec：X 仅内部，Y 写入外部存储。
struct ExampleMeta {
    base: BaseExternalMeta,
    x: i64,
    y: String,
}

impl ExternalMetaCodec for ExampleMeta {
    fn marshal_all(&self) -> crate::Result<Vec<u8>> {
        Ok(format!("{{\"X\":{},\"Y\":\"{}\"}}", self.x, self.y).into_bytes())
    }

    fn marshal_internal(&self) -> crate::Result<Vec<u8>> {
        Ok(format!("{{\"X\":{}}}", self.x).into_bytes())
    }

    fn marshal_external(&self) -> crate::Result<Vec<u8>> {
        Ok(format!("{{\"Y\":\"{}\"}}", self.y).into_bytes())
    }

    fn unmarshal_external(&mut self, data: &[u8]) -> crate::Result<()> {
        let text = std::str::from_utf8(data)
            .map_err(|error| crate::Error::InvalidData(error.to_string()))?;
        let value = text
            .strip_prefix("{\"Y\":\"")
            .and_then(|rest| rest.strip_suffix("\"}"))
            .ok_or_else(|| crate::Error::InvalidData("unexpected external JSON".into()))?;
        self.y = value.to_owned();
        Ok(())
    }
}

/// 校验内外层 JSON 读写：无 ExternalPath 时全量序列化；有路径时外部只存 Y。
#[test]
fn test_read_write_json() {
    let store = MemoryStorage::default();
    let mut meta = ExampleMeta {
        base: BaseExternalMeta::default(),
        x: 42,
        y: "test".to_owned(),
    };

    // ExternalPath empty: Marshal returns every field, matching Go's
    // `json.Marshal(ts)` for a struct with no ExternalPath set yet.
    // ExternalPath 为空时 Marshal 返回全部字段。
    let data = meta.base.Marshal(&meta).expect("marshal all");
    assert_eq!(b"{\"X\":42,\"Y\":\"test\"}".to_vec(), data);

    meta.base.ExternalPath = "/test".to_owned();
    meta.base
        .WriteJSONToExternalStorage(&store, &meta)
        .expect("write external");
    let internal_only = meta.base.Marshal(&meta).expect("marshal internal");
    assert_eq!(b"{\"X\":42}".to_vec(), internal_only);

    let mut restored = ExampleMeta {
        base: BaseExternalMeta {
            ExternalPath: "/test".to_owned(),
        },
        x: 0,
        y: String::new(),
    };
    let path = BaseExternalMeta {
        ExternalPath: "/test".to_owned(),
    };
    path.ReadJSONFromExternalStorage(&store, &mut restored)
        .expect("read external");
    assert_eq!("test", restored.y);
    assert_eq!(
        0, restored.x,
        "external storage only restores the external field"
    );
}

#[derive(Default)]
struct CountingCleanupStorage {
    inner: MemoryStorage,
    scans: std::sync::atomic::AtomicUsize,
    deletes: std::sync::atomic::AtomicUsize,
    fail_scan: bool,
    fail_delete: bool,
}
impl Storage for CountingCleanupStorage {
    fn read(&self, path: &str) -> crate::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write(&self, path: &str, bytes: Vec<u8>) -> crate::Result<()> {
        self.inner.write(path, bytes)
    }
    fn list_prefix(&self, prefix: &str) -> crate::Result<Vec<String>> {
        self.scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_scan {
            return Err(crate::Error::InvalidData("scan failed".into()));
        }
        self.inner.list_prefix(prefix)
    }
    fn delete_files(&self, paths: &[String]) -> crate::Result<()> {
        self.deletes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_delete {
            return Err(crate::Error::InvalidData("delete failed".into()));
        }
        self.inner.delete_files(paths)
    }
}
impl astersql_ingestor_simplesst::writer::WriterSink for CountingCleanupStorage {
    fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        self.write(path, data.to_vec())
            .map_err(|error| error.to_string())
    }
}
#[test]
fn batched_cleanup_scans_writer_files_once_and_preserves_other_tasks() {
    use std::sync::atomic::Ordering;
    let store = std::sync::Arc::new(CountingCleanupStorage::default());
    for dir in ["subtask", "subtask2", "kept"] {
        let mut builder = astersql_ingestor_simplesst::writer::WriterBuilder::new();
        builder.set_memory_size_limit(100).set_prop_keys_distance(3);
        let mut writer = builder.build_with_sink(store.clone(), dir, "0");
        for key in 0..30u8 {
            writer.write_row(&[key], &[key]).unwrap();
        }
        writer.close().unwrap();
    }
    let kept = store
        .inner
        .list_prefix("")
        .unwrap()
        .into_iter()
        .filter(|path| path.contains("/kept/"))
        .collect::<Vec<_>>();
    assert!(!kept.is_empty());
    crate::CleanUpFilesInDirectories(store.as_ref(), &[]).unwrap();
    assert_eq!(store.scans.load(Ordering::SeqCst), 0);
    assert_eq!(store.deletes.load(Ordering::SeqCst), 0);
    crate::CleanUpFilesInDirectories(store.as_ref(), &["subtask", "subtask2", "subtask"]).unwrap();
    assert_eq!(store.scans.load(Ordering::SeqCst), 1);
    assert_eq!(store.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(store.inner.list_prefix("").unwrap(), kept);
    crate::CleanUpFilesInDirectories(store.as_ref(), &["subtask", "subtask2"]).unwrap();
    assert_eq!(store.inner.list_prefix("").unwrap(), kept);
}
#[test]
fn batched_cleanup_propagates_scan_and_delete_errors() {
    use std::sync::atomic::Ordering;
    for (fail_scan, fail_delete, expected, deletes) in [
        (true, false, "scan failed", 0),
        (false, true, "delete failed", 1),
    ] {
        let store = CountingCleanupStorage {
            fail_scan,
            fail_delete,
            ..Default::default()
        };
        store.write("42/data", vec![1]).unwrap();
        assert_eq!(
            crate::CleanUpFilesInDirectories(&store, &["42"])
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(store.deletes.load(Ordering::SeqCst), deletes);
        assert_eq!(store.read("42/data").unwrap(), vec![1]);
    }
}

#[test]
fn divide_merge_sort_exact_targets_and_limits() {
    let files: Vec<String> = (0..4580).map(|i| i.to_string()).collect();
    let groups = DivideMergeSortDataFiles(&files, 8, 1).unwrap();
    let mut expected = vec![250; 16];
    expected.extend([73, 73, 73, 73, 72, 72, 72, 72]);
    assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), expected);
    assert_eq!(groups.concat(), files);
    for (count, nodes, concurrency, succeeds) in [
        (62500, 10, 1, true),
        (62501, 10, 1, false),
        (62750, 1, 1, false),
        (62751, 1, 1, false),
        (248128, 62, 64, false),
        (940000, 2, 17, true),
        (940001, 2, 17, false),
        (32000, 32000, 16, true),
        (1000000, 1000000, 16, true),
    ] {
        let input = vec![String::new(); count];
        let result = DivideMergeSortDataFiles(&input, nodes, concurrency);
        assert_eq!(
            result.is_ok(),
            succeeds,
            "count={count} nodes={nodes} concurrency={concurrency}"
        );
        if let Err(error) = &result {
            assert!(astersql_ingestor_errdef::IsTooManyDataFilesError(error));
            assert_eq!(
                error.to_string(),
                astersql_ingestor_errdef::TooManyDataFiles(
                    count,
                    concurrency,
                    (250 * concurrency).min(4000)
                )
                .Error()
            );
        }
        if let Ok(groups) = result {
            assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), count);
            assert!(
                groups
                    .iter()
                    .all(|g| g.len() <= (250 * concurrency).min(4000))
            );
            let target_count: usize = groups
                .iter()
                .map(|g| splitDataFiles(g, concurrency).len())
                .sum();
            assert!(target_count <= (250 * concurrency).min(4000));
            if count == 62500 {
                assert_eq!(target_count, 250);
            }
            if nodes >= 32000 {
                // Unlike the node count, this capacity is bounded by the accepted plan.
                assert!(groups.capacity() <= groups.len().next_power_of_two());
            }
            if count == 1000000 {
                assert_eq!(groups.len(), 250);
                assert_eq!(target_count, 4000);
            }
        }
    }
    assert!(DivideMergeSortDataFiles(&[], 0, 1).is_err());
    assert!(DivideMergeSortDataFiles(&[], 1, 1).unwrap().is_empty());
}

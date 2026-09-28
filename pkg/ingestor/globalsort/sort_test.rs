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

// 全局排序端到端本地流程测试：写入分片 →（可选）合并 → 读回比对。
//
// 覆盖仅读回、`MergeOverlappingFiles`（V1）与 `MergeOverlappingFilesV2` 两条合并路径。
// 合并（merge-sort）把重叠 key 范围的中间文件归并成更少的有序输出。

// Ported from pkg/ingestor/globalsort/sort_test.go. Go's write step relies
// on `simplesst.Writer` to shard KVs into several SST files automatically;
// this port writes the same sorted KV sequence directly into several
// `encode_kvs` files via `MemoryStorage`, which is what the Rust production
// reader/splitter (reader.rs/split.rs) actually consumes. The merge step
// then drives the real `NewMergeOperator`/`MergeOverlappingFiles` and
// `MergeOverlappingFilesV2` production entry points, and the final
// round-trip is checked with `testutil::testReadAndCompare`, exactly as Go's
// `testReadAndCompare` helper does.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::merge::{NewMergeCollector, NewMergeOperator, SubtaskSummary};
use crate::merge_v2::MergeOverlappingFilesV2;
use crate::reader::CancellationToken;
use crate::testutil::{mockOneMultiFileStat, testReadAndCompare};
use crate::{
    BytesMax, BytesMin, KvPair, MemoryStorage, OnDuplicateKey, Storage, decode_kvs, encode_kvs,
    next_key,
};

/// 生成 `count` 条确定性键并按键排序，作为全局排序输入夹具。
fn sorted_kvs(count: usize) -> Vec<KvPair> {
    let mut kvs: Vec<KvPair> = (0..count)
        .map(|i| KvPair {
            key: format!("{i:08x}-{:08x}", i.wrapping_mul(2654435761)).into_bytes(),
            value: b"56789".to_vec(),
        })
        .collect();
    kvs.sort_by(|a, b| a.key.cmp(&b.key));
    kvs
}

/// 将有序 KV 均分写入 `file_count` 个 data/stat 文件对。
fn write_chunked_files(
    store: &dyn Storage,
    prefix: &str,
    kvs: &[KvPair],
    file_count: usize,
) -> (Vec<String>, Vec<String>) {
    let chunk_size = kvs.len().div_ceil(file_count.max(1)).max(1);
    let mut data_files = Vec::new();
    let mut stat_files = Vec::new();
    for (index, chunk) in kvs.chunks(chunk_size).enumerate() {
        let data_file = format!("{prefix}/{index}.data");
        let stat_file = format!("{prefix}/{index}.stat");
        store.write(&data_file, encode_kvs(chunk)).unwrap();
        store.write(&stat_file, Vec::new()).unwrap();
        data_files.push(data_file);
        stat_files.push(stat_file);
    }
    (data_files, stat_files)
}

/// 收集合并回调产生的输出文件，并折叠全局 [start, end) 键范围。
///
/// Shared accumulator mirroring the closures Go's tests build inline around
/// `onWriterClose`: it collects every output file and folds the running
/// [start, end) key range across however many times the merge step invokes
/// the callback.
#[derive(Default)]
struct LastStepAccumulator {
    data_files: Vec<String>,
    stat_files: Vec<String>,
    start_key: Option<Vec<u8>>,
    end_key: Option<Vec<u8>>,
}

impl LastStepAccumulator {
    /// 根据一次 `WriterSummary` 更新文件列表与键范围。
    fn record(&mut self, summary: &crate::WriterSummary) {
        for stat in &summary.multiple_files_stats {
            for pair in &stat.filenames {
                self.data_files.push(pair.data_file.clone());
                self.stat_files.push(pair.stat_file.clone());
            }
        }
        let candidate_end = next_key(&summary.max);
        self.start_key = Some(match self.start_key.take() {
            Some(existing) => BytesMin(&existing, &summary.min).to_vec(),
            None => summary.min.clone(),
        });
        self.end_key = Some(match self.end_key.take() {
            Some(existing) => BytesMax(&existing, &candidate_end).to_vec(),
            None => candidate_end,
        });
    }
}

// 仅写读路径：RangeSplitter + read_all_data 应精确往返。
// test_global_sort_local_basic corresponds to Go's TestGlobalSortLocalBasic:
// write a sorted KV sequence, then read it back through the range splitter
// and confirm it round-trips exactly.
#[test]
fn test_global_sort_local_basic() {
    let store = MemoryStorage::default();
    let kvs = sorted_kvs(10_000);
    let (data_files, stat_files) = write_chunked_files(&store, "/test", &kvs, 8);

    let token = CancellationToken::default();
    testReadAndCompare(
        &token,
        &kvs,
        &store,
        &data_files,
        &stat_files,
        kvs[0].key.clone(),
        1024 * 1024,
    )
    .expect("read-and-sort step should reproduce the written KVs");
}

// 分组调用 MergeOverlappingFiles，校验 Collector 统计与合并后往返。
// test_global_sort_local_with_merge corresponds to Go's
// TestGlobalSortLocalWithMerge: after the write step, the produced files are
// grouped and merged through NewMergeOperator/MergeOverlappingFiles (the
// scheduler-style grouping used by add-index/import-into merge-sort steps),
// and the merge Collector's totals plus the final read-and-sort round trip
// are both checked.
#[test]
fn test_global_sort_local_with_merge() {
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let kvs = sorted_kvs(10_000);
    let kv_size: usize = kvs.iter().map(KvPair::encoded_size).sum();
    let (data_files, _stat_files) = write_chunked_files(store.as_ref(), "/test", &kvs, 16);

    // 将写步骤输出按最多 10 个文件一组交给合并步骤。
    // Mirrors Go's splitDataAndStatFiles: chunk the write step's outputs
    // into groups of (at most) 10 files for the merge-sort step.
    let step = 10;
    let accumulator = Arc::new(Mutex::new(LastStepAccumulator::default()));
    let summary = Arc::new(SubtaskSummary::default());

    // 复用同一 MergeOperator，保证 writer-id 单调，避免输出路径冲突。
    // A single operator is reused across groups so its writer-id counter
    // stays monotonic (Go instead mints a fresh `uuid.New()` per merge task);
    // otherwise two groups' outputs would collide on the same "<id>.data"
    // path under `/test2`.
    let accumulator_for_close = Arc::clone(&accumulator);
    let on_writer_close: crate::merge::OnWriterClose = Arc::new(move |summary| {
        accumulator_for_close.lock().unwrap().record(summary);
    });
    let op = NewMergeOperator(
        CancellationToken::default(),
        Arc::clone(&store),
        5 * 1024 * 1024,
        "/test2",
        4096,
        Some(on_writer_close),
        Some(Arc::new(NewMergeCollector(Some(Arc::clone(&summary))))),
        1,
        true,
        OnDuplicateKey::Ignore,
    )
    .expect("merge operator should be created");

    for group_start in (0..data_files.len()).step_by(step) {
        let group_end = (group_start + step).min(data_files.len());
        let group = data_files[group_start..group_end].to_vec();
        crate::merge::MergeOverlappingFiles(&group, &op).expect("merge group should succeed");
    }

    assert_eq!(kvs.len() as i64, summary.row_count.load(Ordering::Relaxed));
    assert_eq!(kv_size as i64, summary.processed.load(Ordering::Relaxed));

    let accumulator = accumulator.lock().unwrap();
    let token = CancellationToken::default();
    testReadAndCompare(
        &token,
        &kvs,
        store.as_ref(),
        &accumulator.data_files,
        &accumulator.stat_files,
        accumulator
            .start_key
            .clone()
            .expect("at least one merge group should have run"),
        1024 * 1024,
    )
    .expect("post-merge read-and-sort step should reproduce the written KVs");
}

// 两两文件经 MergeOverlappingFilesV2 合并后再做读回比对。
// test_global_sort_local_with_merge_v2 corresponds to Go's
// TestGlobalSortLocalWithMergeV2: pairs of write-step files are merged
// through MergeOverlappingFilesV2, and the final output is checked with the
// same read-and-sort round trip.
#[test]
fn test_global_sort_local_with_merge_v2() {
    let store = MemoryStorage::default();
    let kvs = sorted_kvs(10_000);
    let (data_files, stat_files) = write_chunked_files(&store, "/test", &kvs, 16);

    let accumulator = Arc::new(Mutex::new(LastStepAccumulator::default()));

    let mut index = 0;
    let mut writer_id = 0;
    while index < data_files.len() {
        let group_end = (index + 2).min(data_files.len());
        let group_data = data_files[index..group_end].to_vec();
        let group_stat = stat_files[index..group_end].to_vec();
        index = group_end;

        let group_min = decode_kvs(&store.read(&group_data[0]).unwrap(), 0)
            .unwrap()
            .first()
            .expect("group should contain at least one key")
            .key
            .clone();
        let group_max = decode_kvs(&store.read(group_data.last().unwrap()).unwrap(), 0)
            .unwrap()
            .last()
            .expect("group should contain at least one key")
            .key
            .clone();
        let group_end_key = next_key(&group_max);

        let multi_file_stat = mockOneMultiFileStat(&group_data, &group_stat).unwrap();
        let accumulator_for_close = Arc::clone(&accumulator);
        let on_writer_close: crate::merge::OnWriterClose = Arc::new(move |summary| {
            accumulator_for_close.lock().unwrap().record(summary);
        });

        writer_id += 1;
        MergeOverlappingFilesV2(
            &CancellationToken::default(),
            &multi_file_stat,
            &store,
            &group_min,
            &group_end_key,
            5 * 1024 * 1024,
            "/test2",
            &writer_id.to_string(),
            4096,
            100,
            8 * 1024,
            100,
            Some(&on_writer_close),
            1,
            true,
        )
        .expect("merge v2 group should succeed");
    }

    let accumulator = accumulator.lock().unwrap();
    let token = CancellationToken::default();
    testReadAndCompare(
        &token,
        &kvs,
        &store,
        &accumulator.data_files,
        &accumulator.stat_files,
        accumulator
            .start_key
            .clone()
            .expect("at least one merge group should have run"),
        1024 * 1024,
    )
    .expect("merge-v2 read-and-sort step should reproduce the written KVs");
}

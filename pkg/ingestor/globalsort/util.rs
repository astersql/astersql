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

// 全局排序通用工具：清理、Mock 引擎、SortedKVMeta、外部元数据编解码与合并分批。
//
// `SortedKVMeta` 汇总已排序 KV 的键范围、体积与文件统计；
// `DivideMergeSortDataFiles` 按节点数与合并并发把 data 文件切成子任务批次。

use crate::{
    ConflictInfo, Error, FilePair, KvPair, MultipleFilesStat, RangeProperty, Result, Storage,
    WriterSummary, encode_kvs, next_key,
};

/// 计划/子任务元数据文件名。
const META_NAME: &str = "meta.json";
/// 合并排序单批文件数步长上限。
const MAX_MERGE_SORT_FILE_COUNT_STEP: usize = 4000;

/// Delete task files from both ordinary and randomly partitioned directories.
pub fn CleanUpFiles(store: &dyn Storage, non_partitioned_dir: &str) -> Result<()> {
    CleanUpFilesInDirectories(store, &[non_partitioned_dir])
}

/// Go accepts several task directories and walks the object store only once.
pub fn CleanUpFilesInDirectories(store: &dyn Storage, non_partitioned_dirs: &[&str]) -> Result<()> {
    if non_partitioned_dirs.is_empty() {
        return Ok(());
    }
    // Preserve the existing Rust single-directory API's slash normalization.
    let dirs: Vec<_> = non_partitioned_dirs
        .iter()
        .map(|dir| dir.trim_matches('/'))
        .collect();
    let names = astersql_ingestor_simplesst::util::GetAllFileNamesFromScan(&dirs, || {
        store.list_prefix("")
    })?;
    store.delete_files(&names)
}

// 按固定小块把 KV 写入多个 data/stat 对，保证测试能覆盖多文件分组。
// Go's MockExternalEngine spreads the KV pairs across several simplesst
// files the same way a real ingest engine would. This mirrors that by
// writing one data/stat file pair per 4-KV chunk (an arbitrary, small
// per-file cap chosen only to guarantee "multiple files" like the Go
// fixture, since callers such as split_test.rs rely on that to exercise
// multi-file grouping).
/// Mock 引擎每个 data 文件包含的 KV 条数。
const MOCK_ENGINE_KVS_PER_FILE: usize = 4;

/// 测试用“外部引擎”：把 keys/values 分块写成多组 data/stat 文件。
pub fn MockExternalEngine(
    storage: &dyn Storage,
    keys: &[Vec<u8>],
    values: &[Vec<u8>],
) -> Result<(Vec<String>, Vec<String>)> {
    if keys.len() != values.len() {
        return Err(Error::InvalidArgument(
            "keys and values must have equal length".into(),
        ));
    }
    let pairs: Vec<_> = keys
        .iter()
        .zip(values)
        .map(|(key, value)| KvPair {
            key: key.clone(),
            value: value.clone(),
        })
        .collect();
    let mut data_files = Vec::new();
    let mut stat_files = Vec::new();
    for (index, chunk) in pairs.chunks(MOCK_ENGINE_KVS_PER_FILE).enumerate() {
        let data_file = format!("/mock-test/{index}.data");
        let stat_file = format!("/mock-test/{index}.stat");
        storage.write(&data_file, encode_kvs(chunk))?;
        let (first, last) = (
            chunk.first().expect("chunk is never empty"),
            chunk.last().expect("chunk is never empty"),
        );
        let mut stat_bytes = Vec::new();
        stat_bytes.extend_from_slice(&(first.key.len() as u32).to_le_bytes());
        stat_bytes.extend_from_slice(&first.key);
        stat_bytes.extend_from_slice(&(last.key.len() as u32).to_le_bytes());
        stat_bytes.extend_from_slice(&last.key);
        storage.write(&stat_file, stat_bytes)?;
        data_files.push(data_file);
        stat_files.push(stat_file);
    }
    Ok((data_files, stat_files))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 已排序 KV 集合的元数据：键范围、总量与多文件统计、冲突信息。
pub struct SortedKVMeta {
    /// 范围下界（含）。
    pub StartKey: Vec<u8>,
    /// 范围上界（开，通常为 max 的 next_key）。
    pub EndKey: Vec<u8>,
    /// KV 编码总字节数。
    pub TotalKVSize: u64,
    /// KV 条数。
    pub TotalKVCnt: u64,
    /// 组成该元数据的多文件统计列表。
    pub MultipleFilesStats: Vec<MultipleFilesStat>,
    /// 重复键等冲突统计。
    pub ConflictInfo: ConflictInfo,
}

/// 由可选的 `WriterSummary` 构造 `SortedKVMeta`；空摘要得到默认值。
pub fn NewSortedKVMeta(summary: Option<&WriterSummary>) -> SortedKVMeta {
    let Some(summary) = summary else {
        return SortedKVMeta::default();
    };
    if summary.min.is_empty() && summary.max.is_empty() {
        return SortedKVMeta::default();
    }
    SortedKVMeta {
        StartKey: summary.min.clone(),
        EndKey: next_key(&summary.max),
        TotalKVSize: summary.total_size,
        TotalKVCnt: summary.total_count,
        MultipleFilesStats: summary.multiple_files_stats.clone(),
        ConflictInfo: summary.conflict_info.clone(),
    }
}

impl SortedKVMeta {
    /// 合并另一份元数据：扩展键范围、累加计数并拼接文件统计。
    pub fn Merge(&mut self, other: &SortedKVMeta) {
        if other.StartKey.is_empty() && other.EndKey.is_empty() {
            return;
        }
        if self.StartKey.is_empty() && self.EndKey.is_empty() {
            self.clone_from(other);
            return;
        }
        self.StartKey = BytesMin(&self.StartKey, &other.StartKey).to_vec();
        self.EndKey = BytesMax(&self.EndKey, &other.EndKey).to_vec();
        // Go's uint64 addition wraps modulo 2^64; preserve that behavior in
        // debug and release Rust builds instead of saturating at u64::MAX.
        self.TotalKVSize = self.TotalKVSize.wrapping_add(other.TotalKVSize);
        self.TotalKVCnt = self.TotalKVCnt.wrapping_add(other.TotalKVCnt);
        self.MultipleFilesStats
            .extend(other.MultipleFilesStats.iter().cloned());
        self.ConflictInfo.merge(&other.ConflictInfo);
    }

    /// 将 `WriterSummary` 转为元数据后合并进来。
    pub fn MergeSummary(&mut self, summary: Option<&WriterSummary>) {
        self.Merge(&NewSortedKVMeta(summary));
    }

    /// 展开全部 data 文件路径。
    pub fn GetDataFiles(&self) -> Vec<String> {
        self.MultipleFilesStats
            .iter()
            .flat_map(|stat| stat.filenames.iter().map(|pair| pair.data_file.clone()))
            .collect()
    }

    /// 展开全部 stat 文件路径。
    pub fn GetStatFiles(&self) -> Vec<String> {
        self.MultipleFilesStats
            .iter()
            .flat_map(|stat| stat.filenames.iter().map(|pair| pair.stat_file.clone()))
            .collect()
    }
}

/// 返回字典序较小的字节切片引用。
pub fn BytesMin<'a>(a: &'a [u8], b: &'a [u8]) -> &'a [u8] {
    if a < b { a } else { b }
}

/// 返回字典序较大的字节切片引用。
pub fn BytesMax<'a>(a: &'a [u8], b: &'a [u8]) -> &'a [u8] {
    if a > b { a } else { b }
}

/// 外部任务元数据的编解码接口（全量/内部/外部三种形态）。
pub trait ExternalMetaCodec {
    fn marshal_all(&self) -> Result<Vec<u8>>;
    fn marshal_internal(&self) -> Result<Vec<u8>>;
    fn marshal_external(&self) -> Result<Vec<u8>>;
    fn unmarshal_external(&mut self, data: &[u8]) -> Result<()>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 带可选外部路径的元数据基类字段。
pub struct BaseExternalMeta {
    /// 外部存储上的 JSON 路径；空表示仅内存/内部形态。
    pub ExternalPath: String,
}

impl BaseExternalMeta {
    /// 按是否有 ExternalPath 选择 marshal_all 或 marshal_internal。
    pub fn Marshal(&self, alias: &dyn ExternalMetaCodec) -> Result<Vec<u8>> {
        if self.ExternalPath.is_empty() {
            alias.marshal_all()
        } else {
            alias.marshal_internal()
        }
    }

    /// 若配置了 ExternalPath，将外部形态 JSON 写入存储。
    pub fn WriteJSONToExternalStorage(
        &self,
        store: &dyn Storage,
        alias: &dyn ExternalMetaCodec,
    ) -> Result<()> {
        if self.ExternalPath.is_empty() {
            return Ok(());
        }
        store.write(&self.ExternalPath, alias.marshal_external()?)
    }

    /// 若配置了 ExternalPath，从存储读取并 unmarshal_external。
    pub fn ReadJSONFromExternalStorage(
        &self,
        store: &dyn Storage,
        alias: &mut dyn ExternalMetaCodec,
    ) -> Result<()> {
        if self.ExternalPath.is_empty() {
            return Ok(());
        }
        alias.unmarshal_external(&store.read(&self.ExternalPath)?)
    }
}

/// 计划步骤元数据路径：`{taskID}/plan/{step}/{index}/meta.json`。
pub fn PlanMetaPath(taskID: i64, step: &str, index: usize) -> String {
    format!("{taskID}/plan/{step}/{index}/{META_NAME}")
}

/// 已准备阶段元数据路径。
pub fn PreparedMetaPath(taskID: i64) -> String {
    format!("{taskID}/plan/prepared/{META_NAME}")
}

/// 子任务元数据路径。
pub fn SubtaskMetaPath(taskID: i64, subtaskID: i64) -> String {
    format!("{taskID}/{subtaskID}/{META_NAME}")
}

/// 按合并并发调整每批文件步长，上限为 `MAX_MERGE_SORT_FILE_COUNT_STEP`。
fn adjusted_file_count_step(merge_concurrency: usize) -> usize {
    250usize
        .saturating_mul(merge_concurrency.max(1))
        .min(MAX_MERGE_SORT_FILE_COUNT_STEP)
}

/// 合并重叠度阈值，公式与步长调整类似。
fn adjusted_overlap_threshold(merge_concurrency: usize) -> usize {
    250usize
        .saturating_mul(merge_concurrency.max(1))
        .min(MAX_MERGE_SORT_FILE_COUNT_STEP)
}

/// Balance complete node rounds and cap the exact merge output file count.
/// The execution concurrency must match the planning concurrency; resource
/// changes between these steps still require pinning or replanning upstream.
pub fn DivideMergeSortDataFiles(
    data_files: &[String],
    node_count: usize,
    merge_concurrency: usize,
) -> Result<Vec<Vec<String>>> {
    // 节点数为 0 非法；空文件列表返回空批次。
    if node_count == 0 {
        return Err(Error::InvalidArgument("unsupported zero node count".into()));
    }
    if data_files.is_empty() {
        return Ok(Vec::new());
    }
    let max_files = adjusted_file_count_step(merge_concurrency);
    let file_count = data_files.len();
    let full_group_count = file_count / max_files / node_count * node_count;
    // Bound capacity by actual input rather than the potentially enormous node count.
    let mut groups = Vec::new();
    let mut cursor = 0;
    for _ in 0..full_group_count {
        groups.push(data_files[cursor..cursor + max_files].to_vec());
        cursor += max_files;
    }
    let target_count =
        full_group_count * crate::merge::getTargetFileCount(max_files, merge_concurrency);
    let target_limit = adjusted_overlap_threshold(merge_concurrency);
    let remaining = file_count - cursor;
    let too_many = || {
        Error::TooManyDataFiles(astersql_ingestor_errdef::TooManyDataFiles(
            file_count,
            merge_concurrency,
            target_limit,
        ))
    };
    if remaining == 0 {
        if target_count > target_limit {
            return Err(too_many());
        }
        return Ok(groups);
    }
    let max_groups = (remaining / 32).min(node_count).max(1);
    let min_groups = remaining.div_ceil(max_files);
    let group_count = (min_groups..=max_groups)
        .rev()
        .find(|&candidate| {
            target_count
                + crate::merge::getGroupedTargetFileCount(remaining, candidate, merge_concurrency)
                <= target_limit
        })
        .ok_or_else(too_many)?;
    let base = remaining / group_count;
    let extra = remaining % group_count;
    for index in 0..group_count {
        let size = base + usize::from(index < extra);
        groups.push(data_files[cursor..cursor + size].to_vec());
        cursor += size;
    }
    Ok(groups)
}

/// 由写出的 KV 与文件路径构造 `WriterSummary`（含粗粒度 RangeProperty）。
pub fn summary_for_file(data_file: String, stat_file: String, kvs: &[KvPair]) -> WriterSummary {
    let properties = kvs
        .chunks(4)
        .map(|chunk| RangeProperty {
            first_key: chunk.first().expect("chunks are non-empty").key.clone(),
            last_key: chunk.last().expect("chunks are non-empty").key.clone(),
            size: chunk.iter().map(|pair| pair.encoded_size() as u64).sum(),
            keys: chunk.len() as u64,
        })
        .collect();
    WriterSummary {
        min: kvs.first().map_or_else(Vec::new, |pair| pair.key.clone()),
        max: kvs.last().map_or_else(Vec::new, |pair| pair.key.clone()),
        total_size: kvs.iter().map(|pair| pair.encoded_size() as u64).sum(),
        total_count: kvs.len() as u64,
        multiple_files_stats: vec![MultipleFilesStat {
            filenames: vec![FilePair {
                data_file,
                stat_file,
                properties,
            }],
        }],
        conflict_info: ConflictInfo::default(),
    }
}

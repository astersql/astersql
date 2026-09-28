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

// 分布式添加索引（DDL 回填，backfilling）流程中的“归并排序”子任务执行器。
//
// 背景：在使用云存储（如 S3）的分布式索引回填方案中，各个读索引子任务会把
// 扫描出的索引键值对（KV）排序后写成多个外部排序文件。这些文件之间的键范围
// 可能互相重叠（overlapping），直接批量导入存储引擎会导致性能下降。因此需要
// 一个归并排序（merge sort）阶段：把多个可能重叠的有序文件归并成若干键范围
// 互不重叠的大文件，供后续的云端导入（ingest）阶段消费。
//
// 本模块定义：
// - [`MergeSortBackend`]：归并排序底层实现的抽象接口（便于测试替换）；
// - [`MergeSortExecutor`]：归并排序子任务的执行器，负责计算分片大小、调用
//   后端归并、汇总排序结果元数据并写回外部元数据存储；
// - 相关错误类型 [`MergeSortError`] 与 [`MergeBackendError`]。

use crate::backfilling_dist_executor::{
    BackfillSubTaskMeta, ExternalMetaStorage, MetaError, write_external_backfill_subtask_meta,
};
use crate::backfilling_import_cloud::{ImportError, IndexInfo, get_index_info_and_id};
use crate::backfilling_read_index::{SortedKvMeta, SubtaskSummary};

/// 归并排序子任务执行过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeSortError {
    /// 执行器尚未调用 [`MergeSortExecutor::init`] 初始化就开始执行子任务。
    NotInitialized,
    /// 当前没有正在运行的子任务（如在空闲时收到资源调整请求）。
    NoSubtaskRunning,
    /// 归并过程中出现的一般性错误，内含错误描述。
    Merge(String),
    /// 归并时发现重复索引键（对唯一索引来说是冲突），
    /// `index_name` 为定位到的索引名称（可能无法定位）。
    DuplicateKey { index_name: Option<String> },
    /// 写外部子任务元数据（存放在云存储中的元信息）失败。
    ExternalMeta(MetaError),
}

/// 归并排序底层后端的抽象接口。
///
/// 真实实现通常封装外部排序引擎（对应 TiDB 的 external sorter），
/// 测试中可以用桩实现替换。
pub trait MergeSortBackend {
    /// 将一组键范围可能互相重叠的有序 KV 文件归并成互不重叠的输出文件。
    ///
    /// - `files`：待归并的输入文件路径列表；
    /// - `part_size`：单个输出分片的目标大小（字节）；
    /// - `output_prefix`：输出文件在云存储中的路径前缀；
    /// - `concurrency`：归并并发度（工作线程数）。
    ///
    /// 成功时返回每个输出批次的排序 KV 元数据（键范围、总大小等）。
    fn merge_overlapping_files(
        &mut self,
        files: &[String],
        part_size: u64,
        output_prefix: &str,
        concurrency: usize,
    ) -> Result<Vec<SortedKvMeta>, MergeBackendError>;
    /// 动态调整工作线程池大小；`wait` 表示是否等待调整完成。
    fn tune_worker_pool_size(&mut self, concurrency: usize, wait: bool);
    /// 返回当前工作线程池大小。
    fn worker_pool_size(&self) -> usize;
}

/// 归并后端返回的错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeBackendError {
    /// 归并时检测到重复键（唯一索引冲突）。
    DuplicateKey,
    /// 其他错误，内含描述文本。
    Other(String),
}

/// 归并排序子任务执行器（对应 TiDB 中的 `mergeSortExecutor`）。
///
/// 泛型参数 `B` 是具体的归并后端实现，需实现 [`MergeSortBackend`]。
pub struct MergeSortExecutor<B> {
    /// 分布式框架分配的任务 ID（一个 DDL 作业可拆成多个任务）。
    pub task_id: i64,
    /// 所属 DDL 作业（job）的 ID。
    pub job_id: i64,
    /// 本次回填涉及的索引信息列表，用于在重复键错误时定位索引名。
    pub indexes: Vec<IndexInfo>,
    /// 云存储（外部存储）的 URI，归并结果文件写入该位置。
    pub cloud_storage_uri: String,
    /// 归并排序后端实现。
    pub backend: B,
    /// 子任务执行统计信息汇总（行数、字节数等）。
    pub summary: SubtaskSummary,
    /// 当前子任务归并产出的排序 KV 元数据；写入外部元数据后清空。
    pub subtask_sorted_kv_meta: Option<SortedKvMeta>,
    /// 是否有子任务正在运行（用于资源动态调整时的状态判断）。
    pub running: bool,
    /// 是否已完成初始化（`init` 是否被调用过）。
    initialized: bool,
}

impl<B: MergeSortBackend> MergeSortExecutor<B> {
    /// 创建一个新的归并排序执行器，初始状态为未初始化、无运行子任务。
    pub fn new(
        task_id: i64,
        job_id: i64,
        indexes: Vec<IndexInfo>,
        cloud_storage_uri: String,
        backend: B,
    ) -> Self {
        Self {
            task_id,
            job_id,
            indexes,
            cloud_storage_uri,
            backend,
            summary: SubtaskSummary::default(),
            subtask_sorted_kv_meta: None,
            running: false,
            initialized: false,
        }
    }

    /// 初始化执行器；必须在 [`run_subtask`](Self::run_subtask) 之前调用。
    pub fn init(&mut self) {
        self.initialized = true;
    }

    /// 执行一个归并排序子任务。
    ///
    /// 流程：
    /// 1. 根据每核内存估算输出分片大小 `part_size`；
    /// 2. 调用后端把子任务元数据 `meta` 中记录的重叠数据文件归并成
    ///    互不重叠的文件；
    /// 3. 将各批次的排序 KV 元数据合并成一份，写回 `meta.meta_groups`；
    /// 4. 把更新后的子任务元数据写入外部元数据存储，供后续导入阶段读取。
    ///
    /// - `memory_per_core`：每个 CPU 核可用的内存字节数，用于估算分片大小；
    /// - `concurrency`：归并并发度；
    /// - `external_storage`：外部元数据存储（云存储上的元信息读写接口）。
    pub fn run_subtask(
        &mut self,
        subtask_id: i64,
        meta: &mut BackfillSubTaskMeta,
        memory_per_core: u64,
        concurrency: usize,
        external_storage: Option<&mut dyn ExternalMetaStorage>,
    ) -> Result<(), MergeSortError> {
        if !self.initialized {
            return Err(MergeSortError::NotInitialized);
        }
        // 分片大小取 5MiB 与「每核内存 * 0.0008」中的较大者：
        // 既保证分片不至于过小（避免产生过多小文件），又与可用内存成正比。
        let part_size = (5_u64 << 20).max(memory_per_core.saturating_mul(8) / 10_000);
        // 输出文件前缀按「任务 ID/子任务 ID」组织，避免不同子任务互相覆盖。
        let prefix = format!("{}/{}", self.task_id, subtask_id);
        self.running = true;
        let merge_result =
            self.backend
                .merge_overlapping_files(&meta.data_files, part_size, &prefix, concurrency);
        self.running = false;
        let summaries = match merge_result {
            Ok(summaries) => summaries,
            Err(MergeBackendError::DuplicateKey) => {
                // 唯一索引出现重复键：尽力根据元素 ID 找到对应索引名，
                // 便于向用户报告是哪个索引冲突。
                let index = get_index_info_and_id(&meta.element_ids, &self.indexes)
                    .ok()
                    .and_then(|(index, _)| index);
                return Err(MergeSortError::DuplicateKey {
                    index_name: index.map(|value| value.name.clone()),
                });
            }
            Err(MergeBackendError::Other(error)) => return Err(MergeSortError::Merge(error)),
        };
        // 把各输出批次的统计元数据（键范围、大小等）合并为一份整体元数据。
        let mut merged = SortedKvMeta::default();
        for summary in summaries {
            merged.merge(&summary);
        }
        self.subtask_sorted_kv_meta = Some(merged.clone());
        meta.meta_groups = vec![merged];
        // Go clears the finished subtask cache before persisting external meta,
        // including when that persistence subsequently fails.
        self.subtask_sorted_kv_meta = None;
        write_external_backfill_subtask_meta(
            external_storage,
            meta,
            format!("{}/{}/meta", self.task_id, subtask_id),
        )
        .map_err(MergeSortError::ExternalMeta)?;
        Ok(())
    }

    /// 清理执行器状态：标记为未运行并丢弃缓存的排序 KV 元数据。
    pub fn cleanup(&mut self) {
        self.running = false;
        self.subtask_sorted_kv_meta = None;
    }
    /// 重置子任务统计信息，供下一个子任务重新累计。
    pub fn reset_summary(&mut self) {
        self.summary.reset();
    }

    /// 响应运行时资源调整（如分布式框架动态调低/调高并发度）。
    ///
    /// 仅在有子任务运行时生效；若目标并发度与当前线程池大小不同，
    /// 则同步调整后端工作线程池。
    pub fn resource_modified(&mut self, concurrency: usize) -> Result<(), MergeSortError> {
        if !self.running {
            return Err(MergeSortError::NoSubtaskRunning);
        }
        let target = concurrency;
        if target != self.backend.worker_pool_size() {
            self.backend.tune_worker_pool_size(target, true);
        }
        Ok(())
    }
}

/// 允许把云端导入阶段的错误（`ImportError`）转换为归并错误，统一错误处理。
impl From<ImportError> for MergeSortError {
    fn from(error: ImportError) -> Self {
        Self::Merge(format!("{error:?}"))
    }
}

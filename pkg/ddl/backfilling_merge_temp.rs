// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 临时索引合并（merge temp index）子任务执行器。
//
// 背景：在分布式 DDL 添加索引的流程中，索引回填（backfill，即为存量数据
// 补建索引条目）期间新写入的数据会先记录到"临时索引"（temporary index）
// 区域，避免与批量回填互相干扰。当回填完成后，需要把临时索引中的增量
// 记录合并回正式索引，本模块即负责这一"合并临时索引"阶段的子任务执行。
//
// 主要内容：
// - [`PhysicalTableCatalog`] / [`TemporaryIndexInfo`]：描述目标物理表
//   （分区表的父表与各分区）及待合并索引的元数据；
// - [`MergeTemporaryIndexExecutor`]：子任务执行器，按子任务元数据定位
//   索引与物理表，然后按 key 范围分批扫描并合并临时索引记录；
// - [`MergeTemporaryIndexError`]：合并过程中的错误类型。

use std::collections::BTreeMap;

use crate::backfilling_dist_executor::BackfillSubTaskMeta;
use crate::backfilling_import_cloud::IndexInfo;
use crate::backfilling_operators::{
    MergeTemporaryIndexWorker, OperatorError, TemporaryIndexRecord, TemporaryIndexResult,
    TemporaryIndexScanTask, TemporaryIndexStore,
};
use crate::backfilling_read_index::SubtaskSummary;

/// 物理表目录：描述一次合并任务涉及的物理表结构。
///
/// 对于分区表，`parent_physical_id` 是父表（逻辑表）的物理 ID，
/// `partition_ids` 是各分区的物理 ID；非分区表则只有父表 ID 本身。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PhysicalTableCatalog {
    /// 父表（或非分区表自身）的物理表 ID。
    pub parent_physical_id: i64,
    /// 分区表的所有分区物理 ID 列表；非分区表为空。
    pub partition_ids: Vec<i64>,
    /// 该表上所有待合并的临时索引信息。
    pub indexes: Vec<TemporaryIndexInfo>,
}

/// 临时索引信息：正在通过临时索引方式回填的索引元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TemporaryIndexInfo {
    /// 索引本身的元数据（ID、名称等）。
    pub info: IndexInfo,
    /// 是否为全局索引（global index）：分区表上跨所有分区的单一索引，
    /// 其索引数据统一存放在父表的 key 空间，而非各分区内。
    pub global: bool,
}

/// 合并临时索引过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeTemporaryIndexError {
    /// 子任务元数据中指定的索引 ID 在表目录中不存在。
    IndexNotFound(i64),
    /// 子任务元数据中指定的分区物理 ID 不属于该分区表。
    PartitionNotFound(i64),
    /// 子任务元数据中的 key 范围非法（起始 key 为空或不小于结束 key），
    /// 或扫描过程中 next_key 未推进（可能导致死循环）。
    InvalidMetaRange,
    /// 底层合并算子（operator）返回的错误。
    Operator(OperatorError),
}

/// 临时索引合并子任务执行器。
///
/// 泛型参数 `S` 为临时索引存储后端（[`TemporaryIndexStore`]），
/// 执行器通过内部的 [`MergeTemporaryIndexWorker`] 按 key 范围分批
/// 扫描临时索引记录并合并到正式索引，同时累计行数与冲突等指标。
pub struct MergeTemporaryIndexExecutor<S> {
    /// 分布式任务框架分配的任务 ID。
    pub task_id: i64,
    /// 所属 DDL 作业（job）的 ID。
    pub job_id: i64,
    /// 每批处理的记录条数上限（至少为 1）。
    pub batch_count: usize,
    /// 目标表的物理表目录（父表、分区与索引信息）。
    pub parent_table: PhysicalTableCatalog,
    /// 当前子任务解析出的目标物理表 ID；初始化前为 `None`。
    pub physical_table_id: Option<i64>,
    /// 当前子任务解析出的目标索引信息；初始化前为 `None`。
    pub index_info: Option<TemporaryIndexInfo>,
    /// 子任务级统计汇总（如已合并行数）。
    pub summary: SubtaskSummary,
    /// 累计扫描的总行数。
    pub total_rows: i64,
    /// 合并计数指标：按 (物理表 ID, 索引 ID) 维度累计成功合并的条目数。
    pub merge_metrics: BTreeMap<(i64, i64), i64>,
    /// 冲突计数指标：按 (物理表 ID, 索引 ID) 维度累计合并冲突次数。
    pub conflict_metrics: BTreeMap<(i64, i64), i64>,
    /// 实际执行扫描与合并的工作器。
    pub worker: MergeTemporaryIndexWorker<S>,
}

impl<S: TemporaryIndexStore> MergeTemporaryIndexExecutor<S> {
    /// 创建执行器。
    ///
    /// `batch_count` 会被钳制为至少 1，避免出现空批次；
    /// 工作器的最大重试次数固定为 16。
    pub fn new(
        task_id: i64,
        job_id: i64,
        batch_count: usize,
        parent_table: PhysicalTableCatalog,
        store: S,
    ) -> Self {
        Self {
            task_id,
            job_id,
            batch_count,
            parent_table,
            physical_table_id: None,
            index_info: None,
            summary: SubtaskSummary::default(),
            total_rows: 0,
            merge_metrics: BTreeMap::new(),
            conflict_metrics: BTreeMap::new(),
            worker: MergeTemporaryIndexWorker {
                store,
                batch_count,
                maximum_attempts: 16,
                total_scan_count: 0,
            },
        }
    }

    /// 执行器初始化钩子；当前无需额外准备工作。
    pub fn init(&mut self) {}

    /// 根据子任务元数据初始化执行状态：解析目标索引与目标物理表。
    ///
    /// 校验内容：
    /// - key 范围必须合法（起始 key 非空且严格小于结束 key）；
    /// - 索引 ID 必须存在于表目录中；
    /// - 非全局索引且元数据指向分区时，该分区必须属于本表。
    pub fn initialize_by_meta(
        &mut self,
        meta: &BackfillSubTaskMeta,
    ) -> Result<(), MergeTemporaryIndexError> {
        let start = &meta.legacy_sorted_kv_meta.start_key;
        let end = &meta.legacy_sorted_kv_meta.end_key;
        if start.is_empty() || start >= end {
            return Err(MergeTemporaryIndexError::InvalidMetaRange);
        }
        // Go initializeByMeta uses tablecodec.DecodeTableID and DecodeIndexID on
        // StartKey.  In particular, ElementIDs and PhysicalTableID are not the
        // authority here: the former may describe a different planning phase,
        // while the latter is only used to label metrics.
        let (decoded_physical_id, requested_index_id) =
            decode_temporary_index_key(start).ok_or(MergeTemporaryIndexError::InvalidMetaRange)?;
        let index = self
            .parent_table
            .indexes
            .iter()
            .find(|index| index.info.id == requested_index_id)
            .cloned()
            .ok_or(MergeTemporaryIndexError::IndexNotFound(requested_index_id))?;
        // 确定目标物理表：全局索引的数据统一挂在父表下；
        // 普通（分区本地）索引若元数据指向某个分区，则必须校验该分区
        // 确实属于本表，否则报分区未找到。
        let physical_id = if !index.global && !self.parent_table.partition_ids.is_empty() {
            self.parent_table
                .partition_ids
                .contains(&decoded_physical_id)
                .then_some(decoded_physical_id)
                .ok_or(MergeTemporaryIndexError::PartitionNotFound(
                    decoded_physical_id,
                ))?
        } else {
            self.parent_table.parent_physical_id
        };
        self.index_info = Some(index);
        self.physical_table_id = Some(physical_id);
        Ok(())
    }

    /// 执行一个合并子任务：在元数据给定的 key 范围内分批扫描并合并
    /// 临时索引记录，返回每批的合并结果。
    ///
    /// 流程：先按元数据初始化目标索引/物理表，然后从起始 key 开始
    /// 循环调用工作器处理一个个子范围，直到覆盖整个范围或工作器
    /// 报告完成（done）。每批结束后累计合并指标与行数统计。
    pub fn run_subtask(
        &mut self,
        subtask_id: i64,
        meta: &BackfillSubTaskMeta,
        records: &[TemporaryIndexRecord],
    ) -> Result<Vec<TemporaryIndexResult>, MergeTemporaryIndexError> {
        self.initialize_by_meta(meta)?;
        let task = TemporaryIndexScanTask {
            id: usize::try_from(subtask_id).unwrap_or_default(),
            start: meta.legacy_sorted_kv_meta.start_key.clone(),
            end: meta.legacy_sorted_kv_meta.end_key.clone(),
        };
        let mut results = Vec::new();
        let mut next = task.start.clone();
        // 按 key 顺序滚动推进：每轮处理 [next, end) 的一个前缀片段。
        while next < task.end {
            let current = TemporaryIndexScanTask {
                start: next.clone(),
                ..task.clone()
            };
            let result = self
                .worker
                .handle_one_range(&current, records)
                .map_err(MergeTemporaryIndexError::Operator)?;
            // next_key 必须严格前进，否则说明范围推进异常，直接报错
            // 以避免无限循环。
            if result.next_key <= next {
                return Err(MergeTemporaryIndexError::InvalidMetaRange);
            }
            next = result.next_key.clone();
            let index_id = self.index_info.as_ref().expect("initialized index").info.id;
            // Go labels the merge metric with meta.PhysicalTableID, even when a
            // global index is physically merged through the parent table.
            *self
                .merge_metrics
                .entry((meta.physical_table_id, index_id))
                .or_default() += result.add_count as i64;
            self.summary.row_count += result.add_count as i64;
            // mergeTempIndexCollector receives addCount from the result sink;
            // its scanCount (and therefore executor.totalRows) tracks that same
            // value rather than the worker's raw scan count.
            self.total_rows += result.add_count as i64;
            let done = result.done;
            results.push(result);
            if done {
                break;
            }
        }
        Ok(results)
    }

    /// 重置子任务统计汇总，供下一个子任务复用执行器。
    pub fn reset_summary(&mut self) {
        self.summary.reset();
    }
    /// 清理子任务环境；与 Go 一致，当前没有需释放的执行器状态。
    pub fn cleanup(&mut self) {}
    /// 任务元数据变更通知钩子；当前无需处理。
    pub fn task_meta_modified(&mut self) {}
    /// 资源（如并发配额）变更通知钩子；当前无需处理。
    pub fn resource_modified(&mut self) {}
}

/// Decode the `t{tableID}_i{indexID}` header used by tablecodec. Integers in
/// keys are sign-bit-flipped before big-endian encoding so byte order remains
/// numeric order. Temporary index IDs additionally carry a high-bit prefix;
/// Go masks that prefix before looking the index up in table metadata.
fn decode_temporary_index_key(key: &[u8]) -> Option<(i64, i64)> {
    const SIGN_MASK: u64 = 1_u64 << 63;
    const INDEX_ID_MASK: i64 = 0x0000_ffff_ffff_ffff;
    const TABLE_ID_START: usize = 1;
    const INDEX_SEPARATOR_START: usize = TABLE_ID_START + 8;
    const INDEX_ID_START: usize = INDEX_SEPARATOR_START + 2;

    if key.first() != Some(&b't') || key.get(INDEX_SEPARATOR_START..INDEX_ID_START) != Some(b"_i") {
        return None;
    }
    let table_bytes: [u8; 8] = key
        .get(TABLE_ID_START..INDEX_SEPARATOR_START)?
        .try_into()
        .ok()?;
    let index_bytes: [u8; 8] = key
        .get(INDEX_ID_START..INDEX_ID_START + 8)?
        .try_into()
        .ok()?;
    let table_id = (u64::from_be_bytes(table_bytes) ^ SIGN_MASK) as i64;
    let encoded_index_id = (u64::from_be_bytes(index_bytes) ^ SIGN_MASK) as i64;
    Some((table_id, encoded_index_id & INDEX_ID_MASK))
}

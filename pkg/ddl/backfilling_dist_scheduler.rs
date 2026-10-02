// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 分布式回填（backfill）任务的调度器模块。
//
// “回填”指在 DDL（数据定义语言，如 ADD INDEX）执行过程中，为表中已有
// 数据补建索引记录的过程。本模块负责在分布式执行框架（DXF）下把整个
// 回填任务切分为若干子任务（subtask），并规划各阶段的执行计划：
//
// - `LitBackfillScheduler`：调度器本体，决定回填流程的阶段流转
//   （读索引 -> 归并排序 -> 写入并 ingest -> 完成）；
// - `generate_plan_for_physical_table`：按 Region（TiKV 中一段连续
//   key 范围的数据分片）批次生成读索引阶段的子任务计划；
// - `generate_merge_sort_plan` / `split_subtask_meta_for_one_kv_group`：
//   全局排序（global sort，借助云存储对索引 KV 做外部排序）路径下
//   归并排序与写入阶段的计划生成；
// - `generate_temporary_index_plan`：临时索引（先写入临时区，最后合并
//   回正式索引）合并阶段的计划生成。

use crate::backfilling::Key;
use crate::backfilling_dist_executor::{BackfillStep, BackfillSubTaskMeta, BackfillTaskMeta};
use crate::backfilling_read_index::SortedKvMeta;

/// 运行期可动态调整的任务参数修改项。
///
/// 分布式框架允许在任务运行中调整部分参数以控制资源占用。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Modification {
    /// 调整一次回填批处理的行数（batch size）。
    BatchSize(usize),
    /// 调整最大写入速度（限流，单位通常为字节/秒）。
    MaxWriteSpeed(usize),
    /// 未知的修改类型，直接忽略。
    Unknown,
}

/// 轻量（lightning ingest 模式）回填任务调度器。
///
/// 保存任务级别的调度信息，并根据是否启用全局排序、是否需要合并临时
/// 索引来决定各阶段之间的流转顺序。
#[derive(Clone, Debug)]
pub struct LitBackfillScheduler {
    /// 是否启用全局排序：任务元数据中配置了云存储 URI 时为 true，
    /// 索引 KV 会先写到云存储做外部排序，再统一写入存储层。
    pub global_sort: bool,
    /// 是否需要执行“合并临时索引”阶段（把临时索引区数据合并回正式索引）。
    pub merge_temporary_index: bool,
    /// 执行节点的 CPU 核数（用于估算并发度）。
    pub node_cpu: usize,
    /// 执行节点的内存大小（字节）。
    pub node_memory: u64,
    /// 执行节点的磁盘大小（字节）。
    pub node_disk: u64,
    /// 回填任务的元数据（目标表、批大小、限速等）。
    pub task_meta: BackfillTaskMeta,
}

impl LitBackfillScheduler {
    /// 根据任务元数据创建调度器，并填入默认的节点资源估计值。
    pub fn new(task_meta: BackfillTaskMeta) -> Self {
        Self {
            global_sort: !task_meta.cloud_storage_uri.is_empty(),
            merge_temporary_index: task_meta.merge_temporary_index,
            node_cpu: 4,
            node_memory: 16 << 30,
            node_disk: 100 << 30,
            task_meta,
        }
    }

    /// 返回给定阶段之后应进入的下一阶段。
    ///
    /// 流转规则：
    /// - Init：若需合并临时索引则进入 MergeTemporaryIndex，否则进入 ReadIndex；
    /// - ReadIndex：启用全局排序时进入 MergeSort，否则直接完成（本地 ingest）；
    /// - MergeSort -> WriteAndIngest -> Done。
    pub const fn get_next_step(&self, step: BackfillStep) -> BackfillStep {
        match step {
            BackfillStep::Init if self.merge_temporary_index => BackfillStep::MergeTemporaryIndex,
            BackfillStep::Init => BackfillStep::ReadIndex,
            BackfillStep::ReadIndex if self.global_sort => BackfillStep::MergeSort,
            BackfillStep::ReadIndex => BackfillStep::Done,
            BackfillStep::MergeSort => BackfillStep::WriteAndIngest,
            BackfillStep::WriteAndIngest | BackfillStep::MergeTemporaryIndex => BackfillStep::Done,
            BackfillStep::Done => BackfillStep::Done,
        }
    }

    /// 按修改项列表就地更新任务元数据。
    pub fn modify_meta(&mut self, modifications: &[Modification]) {
        for modification in modifications {
            match *modification {
                Modification::BatchSize(size) => self.task_meta.batch_size = size,
                Modification::MaxWriteSpeed(speed) => self.task_meta.max_write_speed = speed,
                Modification::Unknown => {}
            }
        }
    }

    /// 回填任务的错误是否可重试；当前一律视为可重试。
    pub const fn is_retryable_error(&self) -> bool {
        true
    }
}

/// 生成执行计划过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    /// 可用执行节点数为 0。
    NoNodes,
    /// Timestamp allocation failed; the entire region scan must be retried.
    TimestampAllocation(String),
    /// Region scan failed; unlike discontinuity this is not retried.
    RegionScan(String),
    /// Region 列表为空，无法切分范围。
    EmptyRegions,
    /// 相邻 Region 的 key 范围不连续（前一个的结束 key 应等于后一个的起始 key）。
    RegionsNotContinuous { expected: Key, actual: Key },
    /// 起始 key 不小于结束 key，范围非法。
    InvalidRange { start: Key, end: Key },
    /// 元数据分组数量不匹配或索引越界（携带出错的分组下标/数量）。
    EmptyMetaGroup(usize),
    /// 请求的索引 ID 在可用索引列表中不存在。
    IndexNotFound(i64),
}

/// Region 的元信息：一段左闭右开的连续 key 范围 `[start_key, end_key)`。
///
/// Region 是 TiKV 中数据分片的基本单位，回填计划按 Region 边界切分子任务。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionMeta {
    /// Region 起始 key（含）。
    pub start_key: Key,
    /// Region 结束 key（不含）。
    pub end_key: Key,
}

/// 计算每个子任务应包含的 Region 数量（批大小）。
///
/// - 使用本地磁盘时：最多按 3 个节点摊分（避免本地盘写入压力过大），
///   且每批不少于 100 个 Region（但不超过总数）；
/// - 使用云存储时：按全部节点平均摊分，每批上限 4000 个 Region。
pub fn calculate_region_batch(
    total_region_count: usize,
    node_count: usize,
    use_local_disk: bool,
) -> Result<usize, PlanError> {
    if node_count == 0 {
        return Err(PlanError::NoNodes);
    }
    if total_region_count == 0 {
        return Ok(0);
    }
    // 本地盘模式下限制参与计算的节点数，防止批次过小、导入次数过多。
    let node_count = if use_local_disk {
        node_count.min(3)
    } else {
        node_count
    };
    // 向上取整的平均值，保证所有 Region 都能被分配。
    let average = total_region_count.div_ceil(node_count);
    Ok(if use_local_disk {
        average.max(100).min(total_region_count)
    } else {
        average.min(4000)
    })
}

/// 为单个物理表（分区表的一个分区或普通表本身）生成读索引阶段的子任务计划。
///
/// 将表的行数据 key 范围 `[table_start, table_end)` 按 Region 批次切分，
/// 每个批次生成一个 `BackfillSubTaskMeta`；首尾批次的边界会分别对齐到
/// 表范围的起止 key。`alloc_ts` 用于为每个子任务分配读取快照用的时间戳
/// （TSO，全局单调递增的事务时间戳）。
pub fn generate_plan_for_physical_table(
    physical_table_id: i64,
    table_start: &[u8],
    table_end: &[u8],
    regions: Vec<RegionMeta>,
    node_count: usize,
    use_cloud: bool,
    mut alloc_ts: impl FnMut() -> u64,
) -> Result<Vec<BackfillSubTaskMeta>, PlanError> {
    try_generate_plan_for_physical_table(
        physical_table_id,
        table_start,
        table_end,
        regions,
        node_count,
        use_cloud,
        || Ok(alloc_ts()),
    )
}

/// Build one attempt, discarding all metadata if timestamp allocation fails.
pub fn try_generate_plan_for_physical_table(
    physical_table_id: i64,
    table_start: &[u8],
    table_end: &[u8],
    mut regions: Vec<RegionMeta>,
    node_count: usize,
    use_cloud: bool,
    mut alloc_ts: impl FnMut() -> Result<u64, PlanError>,
) -> Result<Vec<BackfillSubTaskMeta>, PlanError> {
    if table_start.is_empty() && table_end.is_empty() {
        return Ok(Vec::new());
    }
    if table_start >= table_end {
        return Err(PlanError::InvalidRange {
            start: table_start.to_vec(),
            end: table_end.to_vec(),
        });
    }
    if regions.is_empty() {
        return Err(PlanError::EmptyRegions);
    }
    // 按起始 key 排序后校验相邻 Region 的范围首尾相接，确保覆盖无空洞。
    regions.sort_by(|left, right| left.start_key.cmp(&right.start_key));
    for pair in regions.windows(2) {
        if pair[0].end_key != pair[1].start_key {
            return Err(PlanError::RegionsNotContinuous {
                expected: pair[0].end_key.clone(),
                actual: pair[1].start_key.clone(),
            });
        }
    }
    let batch_size = calculate_region_batch(regions.len(), node_count, !use_cloud)?;
    let total_batches = regions.len().div_ceil(batch_size);
    let mut plan = Vec::with_capacity(total_batches);
    // 每个批次生成一个子任务：范围为批内首个 Region 的起始 key 到
    // 末个 Region 的结束 key。
    for (batch_index, batch) in regions.chunks(batch_size).enumerate() {
        let mut meta = BackfillSubTaskMeta {
            physical_table_id,
            row_start: batch[0].start_key.clone(),
            row_end: batch
                .last()
                .expect("non-empty region batch")
                .end_key
                .clone(),
            ts: alloc_ts()?,
            ..BackfillSubTaskMeta::default()
        };
        // 首尾批次的边界收敛到表本身的范围，避免扫描到表外数据。
        if batch_index == 0 {
            meta.row_start = table_start.to_vec();
        }
        if batch_index + 1 == total_batches {
            meta.row_end = table_end.to_vec();
        }
        plan.push(meta);
    }
    Ok(plan)
}

/// 一组已排序数据文件的统计信息（外部排序的中间产物）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MultipleFilesStat {
    /// 这批文件中 key 范围相互重叠的最大层数（overlap depth）。
    pub max_overlap: usize,
    /// 数据文件路径列表（位于云存储上）。
    pub data_files: Vec<String>,
}

/// 判断是否可以跳过归并排序阶段。
///
/// 若文件间的最大重叠层数不超过并发度的两倍，说明后续写入阶段可以直接
/// 多路归并读取，无需先做一轮归并排序。
pub fn skip_merge_sort(stats: &[MultipleFilesStat], concurrency: usize) -> bool {
    let maximum = stats.iter().map(|stat| stat.max_overlap).max().unwrap_or(0);
    let adjusted_threshold = concurrency.max(1).saturating_mul(2);
    maximum <= adjusted_threshold
}

/// 生成归并排序（MergeSort）阶段的子任务计划。
///
/// 全局排序路径下，读索引阶段会为每个索引（元素分组）产出一批已排序文件；
/// 若文件重叠层数过高，则将文件按节点数与并发度分组，每组一个子任务做归并。
/// 所有分组都可跳过时返回空计划。
pub fn generate_merge_sort_plan(
    meta_groups: &[SortedKvMeta],
    stats_groups: &[Vec<MultipleFilesStat>],
    element_ids: &[i64],
    node_count: usize,
    concurrency: usize,
) -> Result<Vec<BackfillSubTaskMeta>, PlanError> {
    if stats_groups
        .iter()
        .all(|stats| skip_merge_sort(stats, concurrency))
    {
        return Ok(Vec::new());
    }
    if node_count == 0 {
        return Err(PlanError::NoNodes);
    }
    let mut plan = Vec::new();
    // 每个元数据分组（对应一个索引）单独切分归并子任务。
    for (index, _meta) in meta_groups.iter().enumerate() {
        let stats = stats_groups
            .get(index)
            .ok_or(PlanError::EmptyMetaGroup(index))?;
        let files: Vec<String> = stats
            .iter()
            .flat_map(|stat| stat.data_files.iter().cloned())
            .collect();
        // 目标是让 节点数 x 并发度 个归并任务均分所有文件，每组至少 1 个文件。
        let group_size = files
            .len()
            .div_ceil(node_count.saturating_mul(concurrency.max(1)))
            .max(1);
        for group in files.chunks(group_size) {
            plan.push(BackfillSubTaskMeta {
                data_files: group.to_vec(),
                element_ids: element_ids.get(index).copied().into_iter().collect(),
                ..BackfillSubTaskMeta::default()
            });
        }
    }
    Ok(plan)
}

/// 写入并 ingest 阶段中，一段 key 范围的切分描述。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RangeSplit {
    /// 该段范围的结束 key；为 `None` 表示这是最后一段，结束 key 取整个
    /// KV 分组的结束 key。
    pub end_key: Option<Key>,
    /// 该范围涉及的数据文件。
    pub data_files: Vec<String>,
    /// 该范围涉及的统计文件（记录 key 分布，用于切分与预估）。
    pub stat_files: Vec<String>,
    /// 范围内部的作业切分 key（不含首尾，用于进一步拆分写入作业）。
    pub interior_job_keys: Vec<Key>,
    /// 范围内部的 Region 预切分 key（不含首尾，用于提前 split Region）。
    pub interior_region_keys: Vec<Key>,
}

/// 为单个 KV 分组（一个索引的全部已排序数据）生成写入并 ingest 阶段的子任务。
///
/// 按 `splits` 给出的范围切分依次产出子任务，每个子任务携带：
/// - 首尾补齐后的作业切分 key 与 Region 切分 key；
/// - 对应的数据/统计文件与导入用时间戳；
/// - 按实例数摊分总 KV 大小后的元数据分组。
pub fn split_subtask_meta_for_one_kv_group(
    kv_meta: &SortedKvMeta,
    element_id: i64,
    instance_count: usize,
    import_ts: u64,
    splits: &[RangeSplit],
) -> Result<Vec<BackfillSubTaskMeta>, PlanError> {
    if kv_meta.start_key.is_empty() && kv_meta.end_key.is_empty() {
        return Ok(Vec::new());
    }
    if instance_count == 0 {
        return Err(PlanError::NoNodes);
    }
    // start 逐段推进：每段的起点是上一段的终点，保证范围连续。
    let mut start = kv_meta.start_key.clone();
    let mut plan = Vec::new();
    for split in splits {
        let end = split
            .end_key
            .clone()
            .unwrap_or_else(|| kv_meta.end_key.clone());
        if start >= end {
            return Err(PlanError::InvalidRange { start, end });
        }
        // 内部切分 key 首尾各补上本段的起止 key，形成完整的切分序列。
        let mut range_job_keys = Vec::with_capacity(split.interior_job_keys.len() + 2);
        range_job_keys.push(start.clone());
        range_job_keys.extend(split.interior_job_keys.clone());
        range_job_keys.push(end.clone());
        let mut range_split_keys = Vec::with_capacity(split.interior_region_keys.len() + 2);
        range_split_keys.push(start.clone());
        range_split_keys.extend(split.interior_region_keys.clone());
        range_split_keys.push(end.clone());
        plan.push(BackfillSubTaskMeta {
            range_job_keys,
            range_split_keys,
            data_files: split.data_files.clone(),
            stat_files: split.stat_files.clone(),
            ts: import_ts,
            meta_groups: vec![SortedKvMeta {
                start_key: start.clone(),
                end_key: end.clone(),
                file_count: split.data_files.len(),
                total_kv_size: kv_meta.total_kv_size / instance_count as u64,
            }],
            element_ids: (element_id > 0).then_some(element_id).into_iter().collect(),
            ..BackfillSubTaskMeta::default()
        });
        start = end;
        // end_key 为 None 表示已到达最后一段，提前结束。
        if split.end_key.is_none() {
            break;
        }
    }
    Ok(plan)
}

/// 将多个子任务的元数据分组按位置逐一合并。
///
/// 各子任务的分组数必须一致（每个位置对应同一个索引），合并后返回
/// 汇总的分组列表与首个子任务的元素（索引）ID 列表。
pub fn merge_meta_groups(
    subtasks: &[BackfillSubTaskMeta],
) -> Result<(Vec<SortedKvMeta>, Vec<i64>), PlanError> {
    let Some(first) = subtasks.first() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut merged = vec![SortedKvMeta::default(); first.meta_groups.len()];
    for subtask in subtasks {
        if subtask.meta_groups.len() != merged.len() {
            return Err(PlanError::EmptyMetaGroup(subtask.meta_groups.len()));
        }
        for (target, current) in merged.iter_mut().zip(&subtask.meta_groups) {
            target.merge(current);
        }
    }
    Ok((merged, first.element_ids.clone()))
}

/// 校验请求的索引 ID 均存在于可用索引列表中，返回按请求顺序的 ID 列表。
///
/// 任一 ID 不存在时返回 `PlanError::IndexNotFound`。
pub fn find_index_infos_by_ids(
    available_index_ids: &[i64],
    requested_index_ids: &[i64],
) -> Result<Vec<i64>, PlanError> {
    requested_index_ids
        .iter()
        .map(|id| {
            available_index_ids
                .contains(id)
                .then_some(*id)
                .ok_or(PlanError::IndexNotFound(*id))
        })
        .collect()
}

/// 计算合并临时索引阶段每个子任务应包含的 Region 数（按节点数向上平均）。
pub fn calculate_temporary_index_region_batch(
    total_region_count: usize,
    node_count: usize,
) -> Result<usize, PlanError> {
    if node_count == 0 {
        return Err(PlanError::NoNodes);
    }
    Ok(total_region_count.div_ceil(node_count).max(1))
}

/// 生成合并临时索引阶段的子任务计划。
///
/// 临时索引：在线加索引时增量数据先写入的临时 key 区域，最后需要把这些
/// 数据合并回正式索引区。本函数按 Region 批次切分临时索引的 key 范围
/// `[range_start, range_end)`，首尾批次的边界对齐到该范围的起止 key。
pub fn generate_temporary_index_plan(
    physical_table_id: i64,
    _index_id: i64,
    range_start: Key,
    range_end: Key,
    mut regions: Vec<RegionMeta>,
    node_count: usize,
) -> Result<Vec<BackfillSubTaskMeta>, PlanError> {
    if regions.is_empty() {
        return Err(PlanError::EmptyRegions);
    }
    // 与读索引计划相同：排序后校验 Region 范围连续。
    regions.sort_by(|left, right| left.start_key.cmp(&right.start_key));
    for pair in regions.windows(2) {
        if pair[0].end_key != pair[1].start_key {
            return Err(PlanError::RegionsNotContinuous {
                expected: pair[0].end_key.clone(),
                actual: pair[1].start_key.clone(),
            });
        }
    }
    let batch_size = calculate_temporary_index_region_batch(regions.len(), node_count)?;
    let total_batches = regions.len().div_ceil(batch_size);
    let mut plan = Vec::with_capacity(total_batches);
    for (index, batch) in regions.chunks(batch_size).enumerate() {
        let mut legacy = SortedKvMeta {
            start_key: batch[0].start_key.clone(),
            end_key: batch.last().expect("non-empty batch").end_key.clone(),
            ..SortedKvMeta::default()
        };
        // 首尾批次的边界收敛到临时索引整体范围的起止 key。
        if index == 0 {
            legacy.start_key = range_start.clone();
        }
        if index + 1 == total_batches {
            legacy.end_key = range_end.clone();
        }
        plan.push(BackfillSubTaskMeta {
            physical_table_id,
            legacy_sorted_kv_meta: legacy,
            ..BackfillSubTaskMeta::default()
        });
    }
    Ok(plan)
}

/// Retry a full region scan and publish only a complete successful attempt.
pub fn retry_region_plan(
    mut load_regions: impl FnMut() -> Result<Vec<RegionMeta>, PlanError>,
    mut build: impl FnMut(Vec<RegionMeta>) -> Result<Vec<BackfillSubTaskMeta>, PlanError>,
    mut wait: impl FnMut(std::time::Duration) -> Result<(), PlanError>,
) -> Result<Vec<BackfillSubTaskMeta>, PlanError> {
    for attempt in 0..8 {
        // Reload on every attempt: splits/merges can change the batch boundaries.
        let result = build(load_regions()?);
        match result {
            Err(
                error
                @ (PlanError::RegionsNotContinuous { .. } | PlanError::TimestampAllocation(_)),
            ) => {
                wait(std::time::Duration::from_millis(
                    (200_u64 << attempt).min(2000),
                ))?;
                if attempt == 7 {
                    return Err(error);
                }
            }
            result => return result,
        }
    }
    unreachable!("eight attempts always produce a result")
}

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

// 分布式添加索引任务中“读取并回填索引”（read-index）步骤的执行器实现。
//
// 背景：DDL（数据定义语言，如 ADD INDEX）在为已有大表创建索引时，需要
// 扫描全表已有行数据（回填，backfill），据此生成索引键值对（KV）。为了
// 加速，该过程被拆分为分布式框架（DXF）中的多个子任务（subtask），每个
// 子任务负责一段行键范围。生成的索引 KV 可以直接写入本地 ingest 后端，
// 也可以先上传到云存储（外部排序），再由后续步骤统一导入存储层。
//
// 本模块提供：
// - [`SortedKvMeta`]：一批已排序索引 KV 文件的元信息（键范围、文件数、大小）。
// - [`SubtaskSummary`]：子任务执行的统计汇总（读取/处理字节数、行数等）。
// - [`ReadIndexStepExecutor`]：read-index 步骤执行器，管理读写流水线
//   （pipeline）的生命周期与各索引的汇总元数据。
// - [`table_start_end_key`]：按物理表（分区）解析扫描键范围。
// - [`DistTaskRowCountCollector`]：分布式任务的行数/字节数统计收集器。

use std::collections::BTreeMap;

use crate::backfilling::Key;
use crate::backfilling_txn_executor::expected_ingest_worker_count;

/// 一组已排序索引 KV 数据文件的元信息。
///
/// 回填过程会把生成的索引键值对排序后写成文件（本地或云存储），
/// 该结构记录这批文件覆盖的键范围与规模，供后续导入步骤规划使用。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SortedKvMeta {
    /// 这批 KV 覆盖的最小键（含）。
    pub start_key: Key,
    /// 这批 KV 覆盖的最大键（按约定通常为不含的上界）。
    pub end_key: Key,
    /// 已生成的数据文件个数。
    pub file_count: usize,
    /// 全部 KV 的总字节大小。
    pub total_kv_size: u64,
}

impl SortedKvMeta {
    /// 合并另一份元信息：取两者键范围的并集，并累加文件数与总大小。
    pub fn merge(&mut self, other: &Self) {
        // start_key 为空表示尚未记录任何键；否则取更小的起始键。
        if self.start_key.is_empty()
            || (!other.start_key.is_empty() && other.start_key < self.start_key)
        {
            self.start_key.clone_from(&other.start_key);
        }
        // 结束键取两者中更大的一个，保证范围覆盖双方。
        if other.end_key > self.end_key {
            self.end_key.clone_from(&other.end_key);
        }
        self.file_count = self.file_count.wrapping_add(other.file_count);
        self.total_kv_size = self.total_kv_size.wrapping_add(other.total_kv_size);
    }
}

/// 子任务执行过程的统计汇总。
///
/// 用于上报进度与资源消耗，例如从存储层读取的字节数、
/// 处理（编码为索引 KV）的字节数与行数、对象存储的读写次数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubtaskSummary {
    /// 从数据源读取的字节数。
    pub read_bytes: i64,
    /// 已处理（转换为索引 KV）的字节数。
    pub processed_bytes: i64,
    /// 已处理的行数。
    pub row_count: i64,
    /// 向对象存储（如 S3）写入（PUT）的次数。
    pub object_store_put_count: u64,
    /// 从对象存储读取（GET）的次数。
    pub object_store_get_count: u64,
}

impl SubtaskSummary {
    /// 将所有统计项清零，用于开始新的子任务。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 累加另一份统计汇总的各项计数。
    pub fn merge(&mut self, other: &Self) {
        self.read_bytes = self.read_bytes.wrapping_add(other.read_bytes);
        self.processed_bytes = self.processed_bytes.wrapping_add(other.processed_bytes);
        self.row_count = self.row_count.wrapping_add(other.row_count);
        self.object_store_put_count = self
            .object_store_put_count
            .wrapping_add(other.object_store_put_count);
        self.object_store_get_count = self
            .object_store_get_count
            .wrapping_add(other.object_store_get_count);
    }
}

/// 回填子任务的元数据，描述该子任务负责的扫描范围与产出。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackfillSubtaskMeta {
    /// 物理表 ID。对分区表而言，每个分区都是一张独立的物理表。
    pub physical_table_id: i64,
    /// 待扫描行数据的起始键（含）。
    pub row_start: Key,
    /// 待扫描行数据的结束键。
    pub row_end: Key,
    /// 子任务产出的各索引 KV 元信息（与 element_ids/索引一一对应）。
    pub meta_groups: Vec<SortedKvMeta>,
    /// 本子任务涉及的元素（索引）ID 列表。
    pub element_ids: Vec<i64>,
}

/// 分布式框架中的一个子任务实例。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Subtask {
    /// 所属分布式任务的 ID。
    pub task_id: i64,
    /// 子任务自身的 ID。
    pub id: i64,
    /// 子任务的回填元数据。
    pub meta: BackfillSubtaskMeta,
}

/// 分配给当前步骤的计算资源描述。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StepResource {
    /// 可用的 CPU 核心数，用于推算读/写 worker 数量。
    pub cpu: usize,
}

/// 读写流水线（pipeline）的运行状态。
///
/// 回填采用生产者-消费者流水线：reader worker 扫描行数据并编码为索引 KV，
/// writer worker 负责排序落盘或上传云存储。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PipelineState {
    /// 读取（扫描/编码）worker 数量。
    pub reader_workers: usize,
    /// 写入（落盘/上传）worker 数量。
    pub writer_workers: usize,
    /// 流水线是否正在运行。
    pub running: bool,
    /// 流水线是否已关闭（关闭后不可再调整资源）。
    pub closed: bool,
}

/// 单个索引在 read-index 步骤中的汇总产出。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReadIndexSummary {
    /// 索引 ID。
    pub index_id: i64,
    /// 该索引累计生成的 KV 元信息。
    pub meta: SortedKvMeta,
}

/// read-index 步骤可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadIndexError {
    /// 执行器尚未通过 `init` 初始化。
    NotInitialized,
    /// 流水线不存在或已关闭，无法执行操作。
    PipelineNotRunning,
    /// 子任务的行键范围非法（起始键大于结束键）。
    InvalidRange,
    /// ingest 后端已关闭，无法继续写入。
    BackendClosed,
    /// Local-sort disk probing or admission failed during initialization.
    LocalSortDisk(String),
}

/// read-index 步骤执行器：驱动子任务扫描行数据并生成索引 KV。
///
/// 生命周期：`new` 创建 → `init` 打开后端 → 多次 `run_subtask` 执行子任务
/// （期间可通过 `task_meta_modified`/`resource_modified` 动态调整参数）→
/// `cleanup` 释放资源。
#[derive(Clone, Debug)]
pub struct ReadIndexStepExecutor {
    /// 所属 DDL 作业（job）的 ID。
    pub job_id: i64,
    /// 本次要回填的全部索引 ID。
    pub index_ids: Vec<i64>,
    /// 目标物理表 ID。
    pub physical_table_id: i64,
    /// 分区表时涉及的各分区（物理表）ID。
    pub partition_ids: Vec<i64>,
    /// 平均行大小估计值（字节），用于推算 worker 数量。
    pub average_row_size: usize,
    /// 云存储 URI；非空时启用云存储（外部排序）模式。
    pub cloud_storage_uri: String,
    /// 是否使用云存储保存排序后的索引 KV。
    pub use_cloud_storage: bool,
    /// 每批处理的行数。
    pub batch_size: usize,
    /// 写入速度上限（字节/秒），0 表示不限速。
    pub max_write_speed: usize,
    /// TiDB executor node identifier used in local-sort admission errors.
    pub exec_id: String,
    /// Effective DXF slots whose local-sort headroom must be admitted.
    pub runtime_slots: i32,
    /// 当前子任务的统计汇总。
    pub summary: SubtaskSummary,
    /// 各索引 ID 到其汇总产出的映射。
    pub summary_map: BTreeMap<i64, ReadIndexSummary>,
    /// 当前流水线状态；`None` 表示尚未启动。
    pub pipeline: Option<PipelineState>,
    /// 是否已完成初始化。
    initialized: bool,
    /// ingest 后端是否处于打开状态。
    backend_open: bool,
}

impl ReadIndexStepExecutor {
    /// 创建执行器，填入默认参数（batch_size 默认为 256）。
    pub fn new(job_id: i64, index_ids: Vec<i64>, physical_table_id: i64) -> Self {
        Self {
            job_id,
            index_ids,
            physical_table_id,
            partition_ids: Vec::new(),
            average_row_size: 0,
            cloud_storage_uri: String::new(),
            use_cloud_storage: false,
            batch_size: 256,
            max_write_speed: 0,
            exec_id: String::new(),
            runtime_slots: 0,
            summary: SubtaskSummary::default(),
            summary_map: BTreeMap::new(),
            pipeline: None,
            initialized: false,
            backend_open: false,
        }
    }

    /// 初始化执行器：记录云存储 URI 并打开 ingest 后端。
    /// URI 长度非零即视为启用云存储模式，与 Go 的 `len(uri) > 0` 一致。
    pub fn init(&mut self, cloud_storage_uri: impl Into<String>) -> Result<(), ReadIndexError> {
        self.cloud_storage_uri = cloud_storage_uri.into();
        self.use_cloud_storage = !self.cloud_storage_uri.is_empty();
        if !self.use_cloud_storage && !self.exec_id.is_empty() {
            let path = astersql_ddl_ingest::env::ingest_temp_data_dir().ok_or_else(|| {
                ReadIndexError::LocalSortDisk("ingest environment is not initialized".into())
            })?;
            astersql_ddl_ingest::disk_root::check_local_sort_disk_space_at_path(
                &self.exec_id,
                &path,
                self.runtime_slots,
                100 * 1024 * 1024 * 1024,
            )
            .map_err(ReadIndexError::LocalSortDisk)?;
        }
        self.backend_open = true;
        self.initialized = true;
        Ok(())
    }

    /// Attaches the node and effective task slots supplied by the DXF base executor.
    pub fn set_runtime_context(&mut self, exec_id: impl Into<String>, runtime_slots: i32) {
        self.exec_id = exec_id.into();
        self.runtime_slots = runtime_slots;
    }

    /// 执行一个回填子任务。
    ///
    /// 流程：校验状态与键范围 → 重置统计并按资源启动流水线 →
    /// 云存储模式下把生成的 KV 元信息写回子任务并按索引合并汇总 →
    /// 累加统计并结束流水线。
    ///
    /// `generated_meta` 与 `collected_summary` 是本次子任务实际产出的
    /// KV 元信息与统计数据（由调用方收集后传入）。
    pub fn run_subtask(
        &mut self,
        subtask: &mut Subtask,
        resource: StepResource,
        generated_meta: Vec<SortedKvMeta>,
        collected_summary: SubtaskSummary,
    ) -> Result<(), ReadIndexError> {
        if !self.initialized {
            return Err(ReadIndexError::NotInitialized);
        }
        if !self.backend_open {
            return Err(ReadIndexError::BackendClosed);
        }
        if subtask.meta.row_start > subtask.meta.row_end {
            return Err(ReadIndexError::InvalidRange);
        }

        self.reset_subtask();
        // 根据可用 CPU、平均行大小与存储模式推算读/写 worker 数量。
        let (reader_workers, writer_workers) = expected_ingest_worker_count(
            resource.cpu.max(1),
            self.average_row_size,
            self.use_cloud_storage,
        );
        self.pipeline = Some(PipelineState {
            reader_workers,
            writer_workers,
            running: true,
            closed: false,
        });

        if self.use_cloud_storage {
            // 云存储模式：把生成的 KV 元信息记录到子任务元数据中，
            // 并按索引 ID 合并到全局汇总（供后续导入步骤规划范围）。
            subtask.meta.meta_groups = generated_meta.clone();
            subtask.meta.element_ids.clone_from(&self.index_ids);
            for (index, meta) in self.index_ids.iter().copied().zip(generated_meta) {
                self.summary_map
                    .entry(index)
                    .or_insert_with(|| ReadIndexSummary {
                        index_id: index,
                        meta: SortedKvMeta::default(),
                    })
                    .meta
                    .merge(&meta);
            }
        } else {
            // 本地 ingest 模式：数据直接写入本地后端，无需记录云端元信息。
            subtask.meta.meta_groups.clear();
        }
        self.summary.merge(&collected_summary);
        self.on_finished();
        Ok(())
    }

    /// 任务元数据被修改时的回调：更新批大小（至少为 1）与写速上限。
    pub fn task_meta_modified(&mut self, batch_size: usize, max_write_speed: usize) {
        self.batch_size = batch_size.max(1);
        if !self.use_cloud_storage {
            self.max_write_speed = max_write_speed;
        }
    }

    /// 分配的资源发生变化时的回调：按新资源重新计算 worker 数量。
    /// 流水线不存在或已关闭时返回 [`ReadIndexError::PipelineNotRunning`]。
    pub fn resource_modified(&mut self, resource: StepResource) -> Result<(), ReadIndexError> {
        let Some(pipeline) = self.pipeline.as_mut() else {
            return Err(ReadIndexError::PipelineNotRunning);
        };
        if pipeline.closed {
            return Err(ReadIndexError::PipelineNotRunning);
        }
        let (readers, writers) = expected_ingest_worker_count(
            resource.cpu.max(1),
            self.average_row_size,
            self.use_cloud_storage,
        );
        pipeline.reader_workers = readers;
        pipeline.writer_workers = writers;
        Ok(())
    }

    /// 子任务完成时的回调：停止并关闭流水线。
    pub fn on_finished(&mut self) {
        if let Some(pipeline) = self.pipeline.as_mut() {
            pipeline.running = false;
            pipeline.closed = true;
        }
    }

    /// 清理执行器：关闭流水线与后端，并清空汇总数据。
    pub fn cleanup(&mut self) {
        self.on_finished();
        self.backend_open = false;
        self.initialized = false;
        self.summary_map.clear();
    }

    /// 重置子任务级状态（统计、按索引汇总与流水线），为下一个子任务做准备。
    pub fn reset_subtask(&mut self) {
        self.summary.reset();
        self.summary_map.clear();
        self.pipeline = None;
    }
}

/// 解析给定物理表的扫描键范围。
///
/// 若该物理表 ID 在分区范围映射中存在（即为某个分区），返回该分区的范围；
/// 否则回退到整表范围 `table_range`。
pub fn table_start_end_key(
    physical_table_id: i64,
    partition_ranges: &BTreeMap<i64, (Key, Key)>,
    table_range: (Key, Key),
) -> (Key, Key) {
    partition_ranges
        .get(&physical_table_id)
        .cloned()
        .unwrap_or(table_range)
}

/// 分布式任务的行数/字节数收集器，用于上报进度与监控指标（metrics）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DistTaskRowCountCollector {
    /// 累计的子任务统计汇总。
    pub summary: SubtaskSummary,
    /// 上报到监控指标的累计行数。
    pub metric_row_count: i64,
    /// 集群维度的累计读取字节数（保持 Go 的 `int64` 到 `uint64` 转换语义）。
    pub cluster_read_bytes: u64,
}

impl DistTaskRowCountCollector {
    /// 记录已接收（读取）的字节数。
    pub fn accepted(&mut self, bytes: i64) {
        self.summary.read_bytes = self.summary.read_bytes.wrapping_add(bytes);
        // Go 直接将 int64 转为 uint64；保持相同的二进制补码语义。
        self.cluster_read_bytes = self.cluster_read_bytes.wrapping_add(bytes as u64);
    }

    /// 记录已处理的字节数与行数，并同步更新监控指标行数。
    pub fn processed(&mut self, bytes: i64, row_count: i64) {
        self.summary.processed_bytes = self.summary.processed_bytes.wrapping_add(bytes);
        self.summary.row_count = self.summary.row_count.wrapping_add(row_count);
        self.metric_row_count = self.metric_row_count.wrapping_add(row_count);
    }
}

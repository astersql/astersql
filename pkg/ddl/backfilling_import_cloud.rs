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

// 云端导入（cloud import）执行模块：DDL 回填（backfill）流程中"导入"阶段的实现。
//
// 背景：在分布式执行框架下添加索引时，回填分为多个阶段——先由"读取索引"子任务
// 扫描表数据并把生成的索引键值对（KV）排序后写入云存储（如 S3），再由本模块的
// `CloudImportExecutor` 把这些已排序的 KV 文件作为"外部引擎"（external engine）
// 注册到本地导入后端（lightning local backend），最终批量摄取（ingest）到存储层
// 各个 Region（TiKV 中的数据分片单元）。这种"先排序后导入"的方式绕过常规事务
// 写入路径，大幅提升建索引速度。
//
// 术语说明：
// - 回填（backfill）：为已有数据补建索引条目的过程；
// - 外部引擎（external engine）：数据文件位于云存储、由导入后端按需读取的逻辑引擎；
// - 摄取（ingest）：将排序好的 SST/KV 数据直接写入存储引擎，跳过普通写入流程。

use crate::backfilling::Key;
use crate::backfilling_dist_executor::BackfillSubTaskMeta;
use crate::backfilling_read_index::{SortedKvMeta, SubtaskSummary};

/// 索引的基本信息，导入阶段用它定位目标索引并在冲突报错时给出索引名。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引 ID（表内唯一）。
    pub id: i64,
    /// 索引名称，用于错误提示等场景。
    pub name: String,
    /// 是否唯一索引；唯一索引导入时需要检测重复键。
    pub unique: bool,
}

/// 云端导入过程中可能出现的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportError {
    /// 执行器尚未调用 `init` 初始化。
    NotInitialized,
    /// 找不到本地导入后端（local backend）。
    LocalBackendNotFound,
    /// 按引擎 ID 找不到对应的外部引擎。
    ExternalEngineNotFound(String),
    /// 子任务元信息中的元素 ID 列表不符合预期（多于一个）。
    UnexpectedElementIds(Vec<i64>),
    /// 按 ID 找不到对应的索引信息。
    IndexNotFound(i64),
    /// 引擎尚未开始导入却收到了资源调整请求。
    EngineNotStarted,
    /// 检测到重复键（违反唯一索引约束），附带索引名便于提示用户。
    DuplicateKey { index_name: Option<String> },
    /// 底层导入后端返回的其他错误信息。
    Backend(String),
}

/// 云端导入后端抽象：封装 lightning 本地后端在导入阶段用到的能力，
/// 便于在测试中用桩实现替换真实后端。
pub trait CloudImportBackend {
    /// 依据配置注册并关闭（封口）一个外部引擎，使其进入可导入状态。
    fn close_external_engine(
        &mut self,
        config: &ExternalEngineConfig,
        engine_id: &str,
    ) -> Result<(), String>;
    /// 判断指定 ID 的外部引擎是否存在。
    fn has_external_engine(&self, engine_id: &str) -> bool;
    /// 执行导入：把引擎中的 KV 数据摄取到存储层。
    fn import_engine(&mut self, engine_id: &str) -> Result<(), BackendImportError>;
    /// 动态调整引擎可用的并发与内存资源。
    fn update_engine_resource(&mut self, concurrency: usize, memory: u64) -> Result<(), String>;
    /// 设置写入工作线程并发度。
    fn set_worker_concurrency(&mut self, concurrency: usize);
    /// 查询当前写入工作线程并发度。
    fn worker_concurrency(&self) -> usize;
    /// 更新写入限速（字节/秒），用于控制导入对在线业务的影响。
    fn update_write_speed_limit(&mut self, bytes_per_second: usize);
    /// 关闭后端并释放资源。
    fn close(&mut self);
}

/// 后端导入操作的错误分类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendImportError {
    /// 导入时发现重复键（唯一索引冲突）。
    DuplicateKey,
    /// 其他后端错误，携带原始错误文本。
    Other(String),
}

/// 外部引擎的配置：描述云存储上的数据文件及导入相关参数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExternalEngineConfig {
    /// 云存储上的 KV 数据文件列表。
    pub data_files: Vec<String>,
    /// 与数据文件配套的统计文件列表（记录键分布，辅助切分）。
    pub stat_files: Vec<String>,
    /// 本引擎覆盖的键区间起点。
    pub start_key: Key,
    /// 本引擎覆盖的键区间终点。
    pub end_key: Key,
    /// 导入作业级别的切分键，用于把导入任务拆分为多个作业。
    pub job_keys: Vec<Key>,
    /// Region 切分键：导入前按这些键预切分 Region，避免热点。
    pub split_keys: Vec<Key>,
    /// 数据文件总大小（字节），用于进度与资源估算。
    pub total_file_size: u64,
    /// 写入 KV 使用的时间戳（TSO，全局授时产生的版本号）。
    pub timestamp: u64,
    /// 引擎可用的内存容量上限。
    pub memory_capacity: u64,
    /// 是否检查并规避写入热点 Region。
    pub check_hotspot: bool,
    /// 遇到重复键是否视为错误（唯一索引场景为 true）。
    pub duplicate_key_is_error: bool,
}

/// 云端导入执行器：负责把云存储上已排序的索引 KV 数据导入存储层。
/// 泛型参数 `B` 为导入后端实现，生产环境为 lightning 本地后端。
pub struct CloudImportExecutor<B> {
    /// 所属 DDL 作业 ID。
    pub job_id: i64,
    /// 目标表名，参与构造引擎 ID。
    pub table_name: String,
    /// 本次回填涉及的索引列表。
    pub indexes: Vec<IndexInfo>,
    /// 云存储 URI（如 s3://...），数据文件的存放位置。
    pub cloud_storage_uri: String,
    /// 导入后端实例。
    pub backend: B,
    /// 子任务执行统计（读写字节数等汇总信息）。
    pub summary: SubtaskSummary,
    /// 当前写入限速（字节/秒），0 表示不限速。
    pub max_write_speed: usize,
    /// 引擎是否正处于导入运行状态。
    pub engine_running: bool,
    // 标记是否已完成初始化；未初始化时拒绝执行子任务。
    initialized: bool,
}

impl<B: CloudImportBackend> CloudImportExecutor<B> {
    /// 创建执行器实例，此时尚未初始化，需再调用 `init`。
    pub fn new(
        job_id: i64,
        table_name: String,
        indexes: Vec<IndexInfo>,
        cloud_storage_uri: String,
        backend: B,
    ) -> Self {
        Self {
            job_id,
            table_name,
            indexes,
            cloud_storage_uri,
            backend,
            summary: SubtaskSummary::default(),
            max_write_speed: 0,
            engine_running: false,
            initialized: false,
        }
    }

    /// 初始化执行器：按框架资源值设置后端并发度并标记就绪。
    pub fn init(&mut self, concurrency: usize) {
        self.backend.set_worker_concurrency(concurrency);
        self.initialized = true;
    }

    /// 执行一个导入子任务：
    /// 1. 根据子任务元信息定位目标索引，构造引擎 ID；
    /// 2. 合并各元信息组得到整体键区间与数据量；
    /// 3. 注册并关闭外部引擎，然后触发导入（ingest）；
    /// 4. 将后端错误映射为带上下文的 `ImportError`。
    pub fn run_subtask(
        &mut self,
        meta: &BackfillSubTaskMeta,
        memory_capacity: u64,
    ) -> Result<(), ImportError> {
        if !self.initialized {
            return Err(ImportError::NotInitialized);
        }
        let (current_index, index_id) = get_index_info_and_id(&meta.element_ids, &self.indexes)?;
        // 引擎 ID 由表名与索引 ID 组成，保证同表不同索引互不冲突。
        let engine_id = format!("{}-{index_id}", self.table_name);
        // 合并所有元信息组，得到覆盖全部数据的键区间与总 KV 大小。
        let mut all = SortedKvMeta::default();
        for group in &meta.meta_groups {
            all.merge(group);
        }
        // 作业切分键为空时退化为使用 Region 切分键。
        let job_keys = if meta.range_job_keys.is_empty() {
            meta.range_split_keys.clone()
        } else {
            meta.range_job_keys.clone()
        };
        let config = ExternalEngineConfig {
            data_files: meta.data_files.clone(),
            stat_files: meta.stat_files.clone(),
            start_key: all.start_key,
            end_key: all.end_key,
            job_keys,
            split_keys: meta.range_split_keys.clone(),
            total_file_size: all.total_kv_size,
            timestamp: meta.ts,
            memory_capacity,
            check_hotspot: true,
            duplicate_key_is_error: true,
        };
        // 先注册并封口外部引擎，使数据文件对后端可见。
        self.backend
            .close_external_engine(&config, &engine_id)
            .map_err(ImportError::Backend)?;
        if !self.backend.has_external_engine(&engine_id) {
            return Err(ImportError::ExternalEngineNotFound(engine_id));
        }
        // 导入期间置位 engine_running，允许期间的资源动态调整。
        self.engine_running = true;
        let result = self.backend.import_engine(&engine_id);
        self.engine_running = false;
        match result {
            Ok(()) => Ok(()),
            // 重复键错误补充索引名，方便向用户报告哪个唯一索引冲突。
            Err(BackendImportError::DuplicateKey) => Err(ImportError::DuplicateKey {
                index_name: current_index.map(|index| index.name.clone()),
            }),
            Err(BackendImportError::Other(error)) => Err(ImportError::Backend(error)),
        }
    }

    /// 清理执行器：关闭后端并回到未初始化状态。
    pub fn cleanup(&mut self) {
        self.engine_running = false;
        self.backend.close();
        self.initialized = false;
    }
    /// 重置子任务统计信息。
    pub fn reset_summary(&mut self) {
        self.summary.reset();
    }

    /// 任务元信息变更回调：写入限速变化时同步到后端。
    pub fn task_meta_modified(&mut self, new_max_write_speed: usize) {
        if self.max_write_speed != new_max_write_speed {
            self.max_write_speed = new_max_write_speed;
            self.backend.update_write_speed_limit(new_max_write_speed);
        }
    }

    /// 资源变更回调：在导入运行期间动态调整并发与内存。
    /// 并发未变化时直接返回；引擎未运行时无法调整，返回错误。
    pub fn resource_modified(
        &mut self,
        concurrency: usize,
        memory: u64,
    ) -> Result<(), ImportError> {
        if concurrency == self.backend.worker_concurrency() {
            return Ok(());
        }
        if !self.engine_running {
            return Err(ImportError::EngineNotStarted);
        }
        self.backend
            .update_engine_resource(concurrency, memory)
            .map_err(ImportError::Backend)?;
        self.backend.set_worker_concurrency(concurrency);
        Ok(())
    }
}

/// 判断索引列表中是否包含唯一索引；唯一索引导入需开启重复键检测。
pub fn has_unique_index(indexes: &[IndexInfo]) -> bool {
    indexes.iter().any(|index| index.unique)
}

/// 依据子任务的元素 ID 列表解析目标索引信息与索引 ID：
/// - 恰有一个 ID 时按 ID 查找，未找到时保留 Go 命名返回值的零值；
/// - ID 列表为空时取首个索引（仅当索引唯一时返回其信息，否则只返回 ID）；
/// - 多个 ID 属于非预期情况，返回错误。
pub fn get_index_info_and_id<'a>(
    element_ids: &[i64],
    indexes: &'a [IndexInfo],
) -> Result<(Option<&'a IndexInfo>, i64), ImportError> {
    match element_ids {
        [id] => Ok(indexes
            .iter()
            .find(|index| index.id == *id)
            .map_or((None, 0), |index| (Some(index), index.id))),
        [] => {
            // The Go implementation indexes the first entry unconditionally for
            // old-version metadata, so retain that invariant instead of adding a
            // Rust-only recoverable error branch.
            let first = &indexes[0];
            Ok(((indexes.len() == 1).then_some(first), first.id))
        }
        _ => Err(ImportError::UnexpectedElementIds(element_ids.to_vec())),
    }
}

/// 摄取阶段的统计收集器，累计写入集群的字节数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IngestCollector {
    /// 已写入集群的总字节数。
    pub cluster_write_bytes: u64,
}
impl IngestCollector {
    /// 记录一次处理的字节数，保持 Go `uint64(bytes)` 的转换语义。
    pub fn processed(&mut self, bytes: i64) {
        self.cluster_write_bytes = self.cluster_write_bytes.wrapping_add(bytes as u64);
    }
}

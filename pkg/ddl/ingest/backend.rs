// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// ingest 后端上下文模块：管理 DDL（数据定义语言，如建表、加索引等操作）
// 快速加索引（ingest 模式）过程中的写入引擎生命周期。
//
// 背景：传统加索引通过事务逐行回填，速度较慢；ingest 模式则把索引键值
// 先写入本地引擎（内存/磁盘缓冲），再批量导入存储层，大幅提升回填速度。
// 本模块的 `BackendContext` 对应一个 DDL 任务（job）的后端上下文，负责：
// - 按索引 ID 注册/注销写入引擎（`EngineInfo`）；
// - 依据内存与磁盘配额决定何时 flush（刷盘）或 import（导入）；
// - 唯一索引的重复键检测；
// - 通过检查点（checkpoint）记录回填进度，支持故障后断点续传。

use crate::checkpoint::{CheckpointManager, Key};
use crate::disk_root::DiskRoot;
use crate::engine::{Engine, EngineInfo};
use crate::mem_root::MemRoot;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHECKPOINT_UPDATE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// 刷新决策：根据内存/磁盘使用情况判断下一步动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushDecision {
    /// 无需任何操作，继续写入。
    None,
    /// 仅将引擎中的数据刷到本地缓冲（flush），释放内存压力。
    Flush,
    /// 触发导入（import）：把本地数据批量写入存储层。
    Import,
}
/// ingest 后端上下文：一个 DDL 任务（job）对应一个实例，
/// 统一管理该任务下所有索引写入引擎、内存/磁盘配额与检查点进度。
pub struct BackendContext {
    /// 所属 DDL 任务的 ID。
    pub job_id: i64,
    /// 索引 ID 到写入引擎信息的映射，一个索引对应一个引擎。
    pub engines: BTreeMap<i64, Arc<EngineInfo>>,
    /// 内存配额管理器（MemRoot），跟踪引擎的内存使用量。
    pub mem_root: Arc<dyn MemRoot>,
    /// 磁盘配额管理器（DiskRoot），决定是否因磁盘压力触发导入。
    pub disk_root: DiskRoot,
    /// 检查点管理器：记录回填进度（水位线），故障后可断点续传；
    /// 为 `None` 时表示不启用检查点。
    pub checkpoint: Option<CheckpointManager>,
    /// 已触发导入（import）的次数，用于统计与调试。
    pub import_count: usize,
    /// 上下文是否已关闭，关闭后不再允许注册新引擎。
    pub closed: bool,
    /// 是否强制同步导入：为真时下次检查会直接返回 `Import` 决策。
    pub force_sync: bool,
    /// 上次由配额检查触发刷新的时间，用于周期性推进检查点。
    last_flush: Instant,
    /// 周期性刷新检查点的间隔，与 Go `checkpointUpdateInterval` 一致。
    update_interval: Duration,
}
impl BackendContext {
    /// 创建一个新的后端上下文，初始状态下没有任何引擎注册。
    pub fn new(
        job_id: i64,
        mem_root: Arc<dyn MemRoot>,
        disk_root: DiskRoot,
        checkpoint: Option<CheckpointManager>,
    ) -> Self {
        Self {
            job_id,
            engines: BTreeMap::new(),
            mem_root,
            disk_root,
            checkpoint,
            import_count: 0,
            closed: false,
            force_sync: false,
            last_flush: Instant::now(),
            update_interval: CHECKPOINT_UPDATE_INTERVAL,
        }
    }
    /// 为一批索引注册写入引擎。
    ///
    /// `index_ids` 与 `unique` 一一对应，标记每个索引是否为唯一索引；
    /// `writer_memory` 是分配给每个引擎写入器的内存额度（字节）。
    /// 上下文已关闭或两个切片长度不一致时返回错误。
    pub fn register(
        &mut self,
        index_ids: &[i64],
        unique: &[bool],
        writer_memory: i64,
    ) -> Result<Vec<Arc<EngineInfo>>, String> {
        if self.closed {
            return Err("backend closed".into());
        }
        if index_ids.len() != unique.len() {
            return Err("index/unique length mismatch".into());
        }

        // Go Register is idempotent for the same engine group, but rejects a
        // partially overlapping group instead of silently replacing engines.
        let registered = index_ids
            .iter()
            .filter_map(|index_id| self.engines.get(index_id).cloned())
            .collect::<Vec<_>>();
        if !registered.is_empty() {
            if registered.len() != index_ids.len() {
                return Err(format!(
                    "engines index ID number mismatch: job ID {}, required number of index IDs: {}, actual number of engines: {}",
                    self.job_id,
                    index_ids.len(),
                    registered.len()
                ));
            }
            return Ok(registered);
        }
        let mut result = Vec::new();
        // 逐个索引创建引擎并登记到 engines 映射，引擎名包含 job 与索引 ID 便于排查。
        for (&index_id, &is_unique) in index_ids.iter().zip(unique) {
            let engine = Arc::new(EngineInfo::new(
                index_id,
                is_unique,
                format!("job-{}-index-{index_id}", self.job_id),
                Arc::clone(&self.mem_root),
                writer_memory,
            ));
            self.engines.insert(index_id, Arc::clone(&engine));
            result.push(engine);
        }
        Ok(result)
    }
    /// 收集指定唯一索引中的重复行值。
    ///
    /// 唯一索引要求键不重复，导入前需检测冲突；非唯一索引直接返回空。
    /// 返回所有出现次数大于 1 的行值列表。
    pub fn collect_remote_duplicate_rows(&self, index_id: i64) -> Result<Vec<Vec<u8>>, String> {
        let engine = self
            .engines
            .get(&index_id)
            .ok_or_else(|| "engine not found".to_owned())?;
        if !engine.unique() {
            return Ok(Vec::new());
        }
        // 统计每个行值出现的次数，出现超过一次的即为重复。
        let mut seen = BTreeMap::<Vec<u8>, usize>::new();
        for value in engine.rows().values() {
            *seen.entry(value.clone()).or_default() += 1;
        }
        Ok(seen
            .into_iter()
            .filter_map(|(value, count)| (count > 1).then_some(value))
            .collect())
    }
    /// 刷新所有已注册引擎，把内存中的键值数据写入本地缓冲。
    pub fn flush_engines(&self) -> Result<(), String> {
        for engine in self.engines.values() {
            engine.flush()?;
        }
        Ok(())
    }
    /// 根据刷新周期与磁盘压力给出刷新决策：
    /// - 强制同步或磁盘配额告急时立即导入（Import）；
    /// - 距上次刷新达到检查点更新周期时刷盘（Flush）；
    /// - 否则不做任何操作（None）。
    pub fn check_flush(&self) -> FlushDecision {
        if self.force_sync || self.disk_root.should_import() {
            FlushDecision::Import
        } else if self.last_flush.elapsed() >= self.update_interval {
            FlushDecision::Flush
        } else {
            FlushDecision::None
        }
    }
    /// 检查配额并按决策执行动作：返回 `true` 表示本次触发了导入。
    ///
    /// Flush 决策只刷盘并推进未导入的水位线；Import 决策执行完整导入。
    pub fn ingest_if_quota_exceeded(&mut self) -> Result<bool, String> {
        match self.check_flush() {
            FlushDecision::None => Ok(false),
            FlushDecision::Flush => {
                self.flush_engines()?;
                self.last_flush = Instant::now();
                self.advance_watermark(false)?;
                Ok(false)
            }
            FlushDecision::Import => {
                self.ingest()?;
                Ok(true)
            }
        }
    }
    /// 执行一次完整导入：先刷新所有引擎，再累计导入次数，
    /// 并以“已导入”状态推进检查点水位线。
    pub fn ingest(&mut self) -> Result<(), String> {
        self.flush_engines()?;
        self.import_count += 1;
        self.advance_watermark(true)
    }
    /// 结束回填并注销全部引擎。
    ///
    /// `cleanup` 表示关闭引擎时是否清理其本地数据；
    /// `check_duplicates` 为真时先做唯一索引重复键检测，发现重复即报错。
    pub fn finish_and_unregister(
        &mut self,
        cleanup: bool,
        check_duplicates: bool,
    ) -> Result<(), String> {
        self.flush_engines()?;
        if check_duplicates {
            // 先收集索引 ID 快照再逐个检测，避免遍历时借用冲突。
            for index_id in self.engines.keys().copied().collect::<Vec<_>>() {
                if !self.collect_remote_duplicate_rows(index_id)?.is_empty() {
                    return Err(format!("duplicate rows for index {index_id}"));
                }
            }
        }
        for engine in self.engines.values() {
            engine.close(cleanup);
        }
        self.engines.clear();
        Ok(())
    }
    /// 关闭上下文：关闭所有引擎（不清理数据）并标记 closed，
    /// 之后不再接受新的引擎注册。
    pub fn close(&mut self) {
        for engine in self.engines.values() {
            engine.close(false);
        }
        self.closed = true;
    }
    /// 返回下一段回填任务的起始键；未启用检查点时返回空键（从头开始）。
    pub fn next_start_key(&self) -> Key {
        self.checkpoint
            .as_ref()
            .map_or_else(Vec::new, CheckpointManager::next_start_key)
    }
    /// 返回检查点记录的已处理键总数；未启用检查点时为 0。
    pub fn total_key_count(&self) -> u64 {
        self.checkpoint
            .as_ref()
            .map_or(0, CheckpointManager::total_key_count)
    }
    /// 向检查点登记一个新的数据分片（chunk），`end` 为该分片的结束键。
    pub fn add_chunk(&mut self, id: usize, end: Key) {
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint.add_chunk(id, end);
        }
    }
    /// 更新分片进度：`count` 为已处理的键数，`done` 表示该分片是否处理完毕。
    pub fn update_chunk(&mut self, id: usize, count: usize, done: bool) {
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint.update_chunk(id, count, done);
        }
    }
    /// 标记分片处理完成并记录最终键数。
    pub fn finish_chunk(&mut self, id: usize, count: usize) {
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint.finish_chunk(id, count);
        }
    }
    /// 返回导入使用的时间戳（TS，通常来自集群授时服务，保证全局有序）；
    /// 未启用检查点时为 0。
    pub fn import_ts(&self) -> u64 {
        self.checkpoint
            .as_ref()
            .map_or(0, CheckpointManager::import_ts)
    }
    /// 推进检查点水位线（watermark，表示已安全持久化的进度位置）。
    /// `imported` 为真表示数据已导入存储层；未启用检查点时直接成功。
    pub fn advance_watermark(&mut self, imported: bool) -> Result<(), String> {
        self.checkpoint
            .as_mut()
            .map_or(Ok(()), |checkpoint| checkpoint.advance_watermark(imported))
    }
}

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

// DDL ingest（快速导入）检查点管理模块。
//
// 在执行"添加索引"等 DDL reorg（表数据重组）任务时，系统会分批扫描表数据
// 并写入本地排序引擎，再批量导入（ingest）到存储层。为了在任务中断后能够
// 断点续传，本模块维护两级"水位线"（watermark，表示已处理到的键位置）：
// - 本地水位线（local_sync_key）：数据已刷入本地排序引擎的进度；
// - 全局水位线（global_sync_key）：数据已成功导入远端存储的进度。
//
// 检查点会周期性地持久化，重启后可以从最近的水位线继续执行，
// 避免重复处理已完成的键区间。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Current on-disk checkpoint schema version (Go `JobCheckpointVersionCurrent`).
/// 当前检查点的磁盘格式版本号（对应 Go 侧 `JobCheckpointVersionCurrent`），
/// 用于未来格式演进时的兼容性判断。
const JOB_CHECKPOINT_VERSION_CURRENT: u64 = 1;

/// 编码后的行键 / 索引键类型，即字节序列。
/// 键按字节序比较，用于表示扫描进度的区间边界。
pub type Key = Vec<u8>;
/// DDL 任务的 reorg 元信息：记录本次重组的终止键与目标物理表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobReorgMeta {
    pub checkpoint: Option<ReorgCheckpoint>,
}
/// 可持久化的 reorg 检查点内容，对应 Go 侧 `ReorgCheckpoint`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReorgCheckpoint {
    /// 检查点格式版本号，见 `JOB_CHECKPOINT_VERSION_CURRENT`。
    pub version: u64,
    /// 本地水位线：已刷入本地排序引擎（尚未导入远端）的最大键。
    pub local_sync_key: Key,
    /// 已刷新到本地引擎的键数。
    pub local_key_count: u64,
    /// 全局水位线：已成功导入远端存储的最大键，重启后一定可信。
    pub global_sync_key: Key,
    /// 已导入远端存储的键数。
    pub global_key_count: u64,
    /// 产生该检查点的实例地址，用于识别本地排序数据归属的节点。
    pub instance_addr: String,
    /// 当前 reorg 的物理表 ID。
    pub physical_id: i64,
    /// 本次导入使用的时间戳（TS，TSO 全局授时产生），保证导入数据的版本一致。
    pub import_ts: u64,
}
/// 检查点存储抽象：负责检查点的加载与保存。
/// 实际实现可以是系统表（reorg 表）或分布式任务框架的存储。
pub trait CheckpointStorage: Send + Sync {
    /// 加载已保存的检查点；不存在时返回 `Ok(None)`。
    fn load_checkpoint(&self) -> Result<Option<ReorgCheckpoint>, String>;
    /// 持久化保存检查点。
    fn save_checkpoint(&self, checkpoint: &ReorgCheckpoint) -> Result<(), String>;
}
/// 基于内存的检查点存储实现，主要用于测试或单机场景。
/// 内部用 `Arc<Mutex<..>>` 共享一份可选的检查点数据。
#[derive(Clone, Default)]
pub struct MemoryCheckpointStorage(Arc<Mutex<Option<ReorgCheckpoint>>>);
impl CheckpointStorage for MemoryCheckpointStorage {
    fn load_checkpoint(&self) -> Result<Option<ReorgCheckpoint>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save_checkpoint(&self, checkpoint: &ReorgCheckpoint) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(checkpoint.clone());
        Ok(())
    }
}

/// Per-task progress (Go `taskCheckpoint`).
/// 单个扫描任务（按键区间切分）的进度记录，对应 Go 侧 `taskCheckpoint`。
/// 读取端与写入端分别更新各自的计数，两端追平即表示该任务完成。
#[derive(Clone, Debug, Default)]
struct TaskCheckpoint {
    /// 该任务负责区间的终止键，任务完成后用于推进本地水位线。
    end_key: Key,
    /// 读取端累计读到的键数量。
    total_keys: u64,
    /// 写入端累计写入本地排序引擎的键数量。
    written_keys: u64,
    /// 读取端是否已读完最后一批数据。
    last_batch_read: bool,
    /// 读取端产生的批次（chunk）总数。
    chunks_total: u64,
    /// 写入端已完成的批次数。
    chunks_finished: u64,
}

/// In-memory checkpoint manager aligned with Go `ingest.CheckpointManager`.
///
/// `MemoryCheckpointStorage` stands in for the reorg-table / dist-task storage;
/// local-folder presence is modeled by `local_data_is_valid` (true when a
/// checkpoint is resumed from storage).
///
/// 内存态检查点管理器，与 Go 侧 `ingest.CheckpointManager` 对齐。
/// 汇总各扫描任务的读写进度，按任务 ID 顺序推进本地/全局水位线，
/// 并在导入完成或关闭时将检查点持久化到 `CheckpointStorage`。
pub struct CheckpointManager {
    /// 检查点持久化后端。
    storage: Arc<dyn CheckpointStorage>,
    /// 当前内存中的检查点（可能尚未持久化）。
    checkpoint: ReorgCheckpoint,
    /// 进行中的任务进度表，按任务 ID 有序存放（BTreeMap 保证顺序遍历）。
    tasks: BTreeMap<usize, TaskCheckpoint>,
    /// 下一个待确认完成的最小任务 ID；水位线只能按 ID 连续推进。
    min_task_id_finished: usize,
    /// 本地排序数据是否有效：从存储恢复检查点时为 true，
    /// 表示本地水位线之前的数据仍存在于本地排序引擎中。
    local_data_is_valid: bool,
    /// 检查点是否有未持久化的变更。
    dirty: bool,
    /// 管理器是否已关闭（关闭后重复 close 为幂等）。
    closed: bool,
}

impl CheckpointManager {
    /// 创建检查点管理器：优先从存储恢复既有检查点（断点续传），
    /// 否则以 `start_key` 为起点初始化一份全新检查点。
    pub fn new(
        storage: Arc<dyn CheckpointStorage>,
        start_key: Key,
        import_ts: u64,
        instance_addr: impl Into<String>,
    ) -> Result<Self, String> {
        Self::new_with_resume_options(storage, start_key, import_ts, instance_addr, 0, true)
    }

    /// 使用 Go 构造器恢复判断所需的完整上下文创建管理器。
    pub fn new_with_resume_options(
        storage: Arc<dyn CheckpointStorage>,
        start_key: Key,
        import_ts: u64,
        instance_addr: impl Into<String>,
        physical_id: i64,
        local_data_available: bool,
    ) -> Result<Self, String> {
        let instance_addr = instance_addr.into();
        // 恢复到已有检查点时，认为本地排序数据仍然有效；
        // 新建检查点时两级水位线都从起始键开始。
        let (checkpoint, local_data_is_valid) = match storage.load_checkpoint()? {
            Some(mut existing) if existing.physical_id == physical_id => {
                let local_is_valid = local_data_available
                    && (existing.instance_addr == instance_addr
                        || existing.instance_addr.is_empty());
                if !local_is_valid {
                    existing.local_sync_key.clear();
                    existing.local_key_count = 0;
                }
                (existing, local_is_valid)
            }
            None => (
                ReorgCheckpoint {
                    local_sync_key: start_key.clone(),
                    local_key_count: 0,
                    global_sync_key: start_key,
                    global_key_count: 0,
                    instance_addr,
                    physical_id,
                    import_ts,
                    version: 0,
                },
                false,
            ),
            Some(_) => (
                ReorgCheckpoint {
                    local_sync_key: start_key.clone(),
                    local_key_count: 0,
                    global_sync_key: start_key,
                    global_key_count: 0,
                    instance_addr,
                    physical_id,
                    import_ts,
                    version: 0,
                },
                false,
            ),
        };
        Ok(Self {
            storage,
            checkpoint,
            tasks: BTreeMap::new(),
            min_task_id_finished: 0,
            local_data_is_valid,
            dirty: false,
            closed: false,
        })
    }

    /// Go `IsKeyProcessed`: prefer imported (global) watermark; local flushed
    /// watermark only counts when local sort data is still valid.
    ///
    /// 判断以 `end` 结尾的键区间是否已处理过：优先比较全局（已导入）水位线；
    /// 只有当本地排序数据仍有效时，本地（已刷新）水位线才可作为依据。
    pub fn is_key_processed(&self, end: &[u8]) -> bool {
        if !self.checkpoint.global_sync_key.is_empty()
            && end <= self.checkpoint.global_sync_key.as_slice()
        {
            return true;
        }
        self.local_data_is_valid
            && !self.checkpoint.local_sync_key.is_empty()
            && end <= self.checkpoint.local_sync_key.as_slice()
    }

    /// Go `NextStartKey`.
    /// 返回恢复执行时的下一个起始键：本地数据有效时从本地水位线继续，
    /// 否则回退到全局水位线（本地进度已不可信）。
    pub fn next_start_key(&self) -> Key {
        if self.local_data_is_valid && !self.checkpoint.local_sync_key.is_empty() {
            return self.checkpoint.local_sync_key.clone();
        }
        self.checkpoint.global_sync_key.clone()
    }

    /// Go `TotalKeyCount`: flushed keys plus in-flight writer progress.
    /// 总键数 = 已确认刷新的键数 + 各在途任务写入端已写的键数，用于进度展示。
    pub fn total_key_count(&self) -> u64 {
        self.checkpoint.local_key_count
            + self
                .tasks
                .values()
                .map(|task| task.written_keys)
                .sum::<u64>()
    }

    /// Go `AddChunk`.
    /// 注册一个新的扫描任务，记录其终止键，进度计数从零开始。
    pub fn add_chunk(&mut self, task_id: usize, end_key: Key) {
        self.tasks.insert(
            task_id,
            TaskCheckpoint {
                end_key,
                ..TaskCheckpoint::default()
            },
        );
    }

    /// Go `UpdateChunk` (reader side): accumulate total keys / chunk batches.
    /// 读取端上报进度：累加读到的键数与批次数，并记录是否已读到最后一批。
    pub fn update_chunk(&mut self, task_id: usize, delta: usize, last: bool) {
        if let Some(task) = self.tasks.get_mut(&task_id) {
            task.total_keys += delta as u64;
            task.last_batch_read = last;
            task.chunks_total += 1;
        }
    }

    /// Go `FinishChunk` (writer side): accumulate written keys / finished batches.
    /// 写入端上报进度：累加已写入的键数与已完成批次数。
    pub fn finish_chunk(&mut self, task_id: usize, delta: usize) {
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return;
        };
        task.written_keys += delta as u64;
        task.chunks_finished += 1;
    }

    /// Go `AdvanceWatermark`.
    /// 推进水位线：先根据已完成的任务推进本地水位线；
    /// 若数据已成功导入远端（`imported` 为 true），再推进全局水位线并持久化检查点。
    pub fn advance_watermark(&mut self, imported: bool) -> Result<(), String> {
        if self.no_update() {
            return Ok(());
        }
        self.after_flush();
        if imported {
            self.after_import()?;
            self.update_checkpoint()?;
        }
        Ok(())
    }

    /// 刷新完成后推进本地水位线：从最小未完成任务 ID 开始，
    /// 按顺序摘除所有"读写两端均已追平"的任务，把水位线推进到其终止键。
    /// 必须按 ID 连续推进，否则中间存在未完成的键区间，水位线不可跨越。
    fn after_flush(&mut self) {
        loop {
            let Some(task) = self.tasks.get(&self.min_task_id_finished) else {
                break;
            };
            // 任务完成条件：读完最后一批、写入键数追上读取键数、批次数全部完成。
            if !task.last_batch_read
                || task.written_keys < task.total_keys
                || task.chunks_finished < task.chunks_total
            {
                break;
            }
            let task = self
                .tasks
                .remove(&self.min_task_id_finished)
                .expect("task present");
            self.min_task_id_finished += 1;
            self.checkpoint.local_sync_key = task.end_key;
            self.checkpoint.local_key_count += task.total_keys;
            self.dirty = true;
        }
    }

    /// 导入成功后把全局水位线对齐到本地水位线。
    /// 不变量：全局水位线不能超过本地水位线（数据必须先刷新本地再导入远端）。
    fn after_import(&mut self) -> Result<(), String> {
        if self.checkpoint.global_sync_key > self.checkpoint.local_sync_key {
            return Err("flushed key is less than imported key".into());
        }
        self.checkpoint.global_sync_key = self.checkpoint.local_sync_key.clone();
        self.checkpoint.global_key_count = self.checkpoint.local_key_count;
        self.dirty = true;
        Ok(())
    }

    /// 将当前检查点持久化到存储，并清除脏标记。
    fn update_checkpoint(&mut self) -> Result<(), String> {
        self.checkpoint.version = JOB_CHECKPOINT_VERSION_CURRENT;
        self.storage.save_checkpoint(&self.checkpoint)?;
        self.dirty = false;
        Ok(())
    }

    /// Go `noUpdate`: nothing registered and no task has ever completed.
    /// 判断是否毫无进展：既没有注册中的任务，也从未有任务完成过。
    pub fn no_update(&self) -> bool {
        self.tasks.is_empty() && self.min_task_id_finished == 0
    }

    /// Go `Close`: always request a final checkpoint persistence.
    pub fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.update_checkpoint()?;
        self.closed = true;
        Ok(())
    }

    /// 返回本次导入使用的时间戳（TS）。
    pub fn import_ts(&self) -> u64 {
        self.checkpoint.import_ts
    }
}

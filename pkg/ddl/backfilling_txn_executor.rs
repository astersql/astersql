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

// 基于事务（txn）模式的 DDL 回填（backfill）执行器模块。
//
// “回填”指在执行 DDL（数据定义语言，如添加索引、修改列类型）时，
// 需要为表中已有的历史数据补写新索引记录或新列数据的过程。
// 本模块提供 txn 模式下的回填执行框架：
// - `ReorgMeta`：重组（reorganization）任务的元信息（并发度、批大小、限速等）；
// - `SessionContext` / `SessionSnapshot`：回填期间会话上下文的切换与恢复；
// - `TxnBackfillExecutor`：管理回填工作者（worker）池、任务队列与结果队列；
// - `expected_ingest_worker_count`：根据平均行宽估算 ingest（快速导入）模式下
//   读/写工作者的数量；
// - `TaskIdAllocator`：为回填子任务分配单调递增的任务 ID。

use std::collections::VecDeque;

use astersql_distsql_context::{DistSQLContext, WarnAppenderRef, errctx};
use astersql_meta_model::group_3::DDLReorgMeta;

use crate::backfilling::{BackfillResult, BackfillerType, ReorgBackfillTask};

/// Builds the DistSQL context used by transactional reorganization scans.
///
/// Reorganization scans are single-pass bulk reads. Filling TiKV's block cache
/// with those blocks provides no reuse for the DDL job and may evict the
/// working set of concurrent foreground queries.
pub fn new_default_reorg_dist_sql_context(
    warn_handler: WarnAppenderRef,
) -> DistSQLContext<'static> {
    DistSQLContext {
        WarnHandler: warn_handler.clone(),
        EnableChunkRPC: true,
        NotFillCache: true,
        ErrCtx: errctx::NewContext(warn_handler),
        ..DistSQLContext::default()
    }
}

/// Builds a reorganization DistSQL context and applies the persisted job
/// metadata that affects request attribution.
pub fn new_reorg_dist_sql_context_with_reorg_meta(
    reorg_meta: &DDLReorgMeta,
    warn_handler: WarnAppenderRef,
) -> DistSQLContext<'static> {
    DistSQLContext {
        ResourceGroupName: reorg_meta.ResourceGroupName.clone(),
        ..new_default_reorg_dist_sql_context(warn_handler)
    }
}

/// 回填工作者数量的硬上限，避免并发过高导致资源争用。
pub const MAX_BACKFILL_WORKER_SIZE: usize = 16;

/// 重组（reorg）任务的元信息，描述本次回填如何执行。
///
/// “重组”是 DDL 中需要重写或补写存量数据的阶段，例如添加索引时
/// 扫描全表并为每行生成索引键值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorgMeta {
    /// 回填并发度（工作者数量）。
    pub concurrency: usize,
    /// 每个事务批次处理的行数。
    pub batch_size: usize,
    /// 写入限速（字节/秒），0 表示不限速。
    pub max_write_speed: usize,
    /// 是否使用严格 SQL 模式（strict sql mode，影响非法值的报错行为）。
    pub strict_sql_mode: bool,
    /// 回填期间使用的时区名称（影响时间类型的取值计算）。
    pub time_zone: String,
    /// 资源组名称，用于资源管控（resource control）下的限流归属。
    pub resource_group_name: String,
    /// 是否使用云存储进行全局排序（global sort）导入。
    pub use_cloud_storage: bool,
}

impl Default for ReorgMeta {
    /// 默认元信息：单并发、批大小 256、UTC 时区、不限速。
    fn default() -> Self {
        Self {
            concurrency: 1,
            batch_size: 256,
            max_write_speed: 0,
            strict_sql_mode: false,
            time_zone: "UTC".to_owned(),
            resource_group_name: String::new(),
            use_cloud_storage: false,
        }
    }
}

/// 回填期间使用的会话上下文，是完整会话变量的一个简化子集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionContext {
    /// 是否处于严格 SQL 模式。
    pub strict_sql_mode: bool,
    /// 会话时区。
    pub time_zone: String,
    /// 会话所属资源组。
    pub resource_group_name: String,
    /// 每批处理的行数。
    pub batch_size: usize,
}

/// 会话上下文的快照，用于回填结束后恢复原始会话状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSnapshot(SessionContext);

impl SessionContext {
    /// 按重组元信息初始化会话上下文，并返回修改前的快照。
    ///
    /// 回填需要在与用户会话不同的设置（时区、SQL 模式等）下运行，
    /// 因此先保存快照，回填结束后通过 [`SessionContext::restore`] 还原。
    pub fn initialize_for_reorganization(&mut self, meta: &ReorgMeta) -> SessionSnapshot {
        let snapshot = SessionSnapshot(self.clone());
        self.strict_sql_mode = meta.strict_sql_mode;
        self.time_zone.clone_from(&meta.time_zone);
        self.resource_group_name
            .clone_from(&meta.resource_group_name);
        // 批大小至少为 1，避免出现空批导致回填无法推进。
        self.batch_size = meta.batch_size.max(1);
        snapshot
    }

    /// 用快照整体覆盖当前上下文，恢复回填前的会话状态。
    pub fn restore(&mut self, snapshot: SessionSnapshot) {
        *self = snapshot.0;
    }
}

/// 工作者槽位，表示回填工作者池中的一个 worker。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerSlot {
    /// 工作者编号（在池内按顺序分配）。
    pub id: usize,
    /// 回填器类型（如添加索引、更新列等）。
    pub worker_type: BackfillerType,
}

/// 执行器操作可能返回的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutorError {
    /// 执行器已关闭，无法再收发任务或结果。
    Closed,
    /// 并发度非法（为 0）。
    InvalidConcurrency,
}

/// 事务（txn）模式的回填执行器。
///
/// 维护一个工作者池、一个待处理任务队列（`ReorgBackfillTask` 描述
/// 一段待回填的键范围）以及一个结果队列（`BackfillResult` 记录每段
/// 任务的处理结果）。txn 模式表示每批回填都通过普通事务写入存储层，
/// 与 ingest（直接生成 SST 文件导入）模式相对。
#[derive(Debug)]
pub struct TxnBackfillExecutor {
    /// 本执行器创建的工作者类型。
    worker_type: BackfillerType,
    /// 当前工作者池。
    workers: Vec<WorkerSlot>,
    /// 待执行的回填任务队列（先进先出）。
    task_queue: VecDeque<ReorgBackfillTask>,
    /// 已完成任务的结果队列（先进先出）。
    result_queue: VecDeque<BackfillResult>,
    /// 是否已关闭。
    closed: bool,
}

impl TxnBackfillExecutor {
    /// 创建指定回填器类型的空执行器（尚未启动任何工作者）。
    pub fn new(worker_type: BackfillerType) -> Self {
        Self {
            worker_type,
            workers: Vec::new(),
            task_queue: VecDeque::new(),
            result_queue: VecDeque::new(),
            closed: false,
        }
    }

    /// 按给定并发度初始化工作者池；并发度为 0 时返回错误。
    pub fn setup_workers(&mut self, concurrency: usize) -> Result<(), ExecutorError> {
        if concurrency == 0 {
            return Err(ExecutorError::InvalidConcurrency);
        }
        self.adjust_worker_size(concurrency);
        Ok(())
    }

    /// 动态调整工作者数量到目标并发度（限制在 0..=上限之间）。
    ///
    /// 支持在回填过程中在线调节并发：多余的工作者被裁掉，
    /// 不足的按顺序补充新的槽位。
    pub fn adjust_worker_size(&mut self, concurrency: usize) {
        let target = concurrency.min(MAX_BACKFILL_WORKER_SIZE);
        self.workers.truncate(target);
        while self.workers.len() < target {
            self.workers.push(WorkerSlot {
                id: self.workers.len(),
                worker_type: self.worker_type,
            });
        }
    }

    /// 返回当前工作者数量。
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// 向任务队列投递一个回填任务；执行器已关闭时返回错误。
    pub fn send_task(&mut self, task: ReorgBackfillTask) -> Result<(), ExecutorError> {
        if self.closed {
            return Err(ExecutorError::Closed);
        }
        self.task_queue.push_back(task);
        Ok(())
    }

    /// 从任务队列头部取出下一个待处理任务，队列为空时返回 `None`。
    pub fn take_task(&mut self) -> Option<ReorgBackfillTask> {
        self.task_queue.pop_front()
    }

    /// 向结果队列写入一个回填结果；执行器已关闭时返回错误。
    pub fn push_result(&mut self, result: BackfillResult) -> Result<(), ExecutorError> {
        if self.closed {
            return Err(ExecutorError::Closed);
        }
        self.result_queue.push_back(result);
        Ok(())
    }

    /// 从结果队列头部取出下一个结果，队列为空时返回 `None`。
    pub fn result(&mut self) -> Option<BackfillResult> {
        self.result_queue.pop_front()
    }

    /// 关闭执行器并清空工作者池。
    ///
    /// `force` 为 true 时同时丢弃尚未处理的任务与尚未消费的结果，
    /// 用于任务取消等需要立即终止的场景。
    pub fn close(&mut self, force: bool) {
        self.closed = true;
        self.workers.clear();
        if force {
            self.task_queue.clear();
            self.result_queue.clear();
        }
    }

    /// 返回执行器是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// 估算 ingest（快速导入）模式下读、写工作者的数量，返回 `(reader, writer)`。
///
/// ingest 模式将回填拆成“读表数据”和“写索引数据”两个流水线阶段；
/// 行越宽（平均行字节数越大），读取相对越慢，需要更多读工作者来
/// 匹配写入速度。`global_sort` 表示使用云端全局排序，此时读写各用
/// 相同的并发度。
pub fn expected_ingest_worker_count(
    concurrency: usize,
    average_row_size: usize,
    global_sort: bool,
) -> (usize, usize) {
    if global_sort {
        return (concurrency, concurrency);
    }
    // 行宽未知时按经验各取约一半并发，写侧略多以避免写入成为瓶颈。
    if average_row_size == 0 {
        let reader = (concurrency / 2).clamp(1, MAX_BACKFILL_WORKER_SIZE);
        let writer = (concurrency / 2 + 2).clamp(1, MAX_BACKFILL_WORKER_SIZE);
        return (reader, writer);
    }

    // 根据平均行宽选取写/读比例：行越宽，比值越大，即读工作者相对越多。
    let ratio = match average_row_size {
        0..=200 => 0.5,
        201..=500 => 1.0,
        501..=1000 => 2.0,
        1001..=3000 => 4.0,
        _ => 8.0,
    };
    let writer = concurrency.max(1);
    let reader = ((concurrency as f64 * ratio) as usize).max(1);
    (reader, writer)
}

/// 回填任务 ID 分配器，按 0、1、2… 顺序分配单调递增的编号。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskIdAllocator {
    /// 下一个待分配的 ID。
    next_id: usize,
}

impl TaskIdAllocator {
    /// 创建从 0 开始分配的分配器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配并返回当前 ID，随后内部计数器自增。
    pub fn alloc(&mut self) -> usize {
        let allocated = self.next_id;
        self.next_id += 1;
        allocated
    }
}

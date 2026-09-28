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

// 子任务（subtask）写入任务表的测试辅助。
//
// 子任务是 DXF 将一个大任务拆成可并行调度的最小执行单元；
// 本模块封装向 `TaskTable` 插入 Pending 子任务及带 summary 的变体。

use crate::context::{DxfError, Step, SubtaskState};
use crate::table_util::TaskTable;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 插入子任务时使用的字段集合。
pub struct NewSubtask {
    /// 所属任务 ID。
    pub task_id: i64,
    /// 任务步骤（step：调度状态机中的阶段编号）。
    pub step: Step,
    /// 执行节点 ID（通常形如 `:4000`）。
    pub exec_id: String,
    /// 子任务元数据（序列化后的业务载荷）。
    pub meta: Vec<u8>,
    /// 子任务状态（Pending/Running/Succeed 等）。
    pub state: SubtaskState,
    /// 任务类型名，用于查找对应执行器。
    pub task_type: String,
    /// 并发度提示。
    pub concurrency: usize,
    /// 可选的执行摘要 JSON。
    pub summary_json: Option<Vec<u8>>,
    /// 是否写入开始时间戳。
    pub has_start_time: bool,
}

#[allow(non_snake_case)]
/// 以 Pending 状态创建子任务，返回新子任务 ID。
pub fn CreateSubTask(
    manager: &dyn TaskTable,
    task_id: i64,
    step: Step,
    exec_id: &str,
    meta: Vec<u8>,
    task_type: &str,
    concurrency: usize,
) -> Result<i64, DxfError> {
    InsertSubtask(
        manager,
        task_id,
        step,
        exec_id,
        meta,
        SubtaskState::Pending,
        task_type,
        concurrency,
    )
}

#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
/// 按给定状态插入子任务（无 summary）。
pub fn InsertSubtask(
    manager: &dyn TaskTable,
    task_id: i64,
    step: Step,
    exec_id: &str,
    meta: Vec<u8>,
    state: SubtaskState,
    task_type: &str,
    concurrency: usize,
) -> Result<i64, DxfError> {
    manager.insert_subtask(NewSubtask {
        task_id,
        step,
        exec_id: exec_id.to_owned(),
        meta,
        state,
        task_type: task_type.to_owned(),
        concurrency,
        summary_json: None,
        has_start_time: false,
    })
}

#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
/// 创建带 summary 的子任务（会标记已有 start_time）。
pub fn CreateSubTaskWithSummary(
    manager: &dyn TaskTable,
    task_id: i64,
    step: Step,
    exec_id: &str,
    meta: Vec<u8>,
    summary_json: Vec<u8>,
    state: SubtaskState,
    task_type: &str,
    concurrency: usize,
) -> Result<i64, DxfError> {
    InsertSubtaskWithSummary(
        manager,
        task_id,
        step,
        exec_id,
        meta,
        summary_json,
        state,
        task_type,
        concurrency,
    )
}

#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
/// 插入带 summary 的子任务记录。
pub fn InsertSubtaskWithSummary(
    manager: &dyn TaskTable,
    task_id: i64,
    step: Step,
    exec_id: &str,
    meta: Vec<u8>,
    summary_json: Vec<u8>,
    state: SubtaskState,
    task_type: &str,
    concurrency: usize,
) -> Result<i64, DxfError> {
    manager.insert_subtask(NewSubtask {
        task_id,
        step,
        exec_id: exec_id.to_owned(),
        meta,
        state,
        task_type: task_type.to_owned(),
        concurrency,
        summary_json: Some(summary_json),
        has_start_time: true,
    })
}

#[allow(non_snake_case)]
/// 返回任务使用的 keyspace（键空间）名。
///
/// next-gen 内核使用 `SYSTEM`，经典模式返回空串。
pub fn getTaskKS(next_generation_kernel: bool) -> &'static str {
    // next-gen 与 classic 内核的任务表 keyspace 约定不同。
    if next_generation_kernel { "SYSTEM" } else { "" }
}

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

// 任务状态机合法迁移校验。
//
// DXF（Distributed eXecution Framework，分布式执行框架）中，任务（Task）
// 按状态机推进生命周期。本模块提供 `verify_task_state_transform`，判断
// 从状态 `from` 到 `to` 是否为框架允许的一次迁移，防止调度器误写非法状态。
//
// 术语：Reverting/Reverted 表示失败后的回滚收尾；Pausing/Paused/Resuming
// 表示暂停与恢复链；Cancelling 表示用户取消后转入回滚。

use crate::interface::*;

/// 校验任务状态从 `from` 迁移到 `to` 是否合法。
///
/// 相同状态视为合法（幂等刷新）。其余分支按 DXF 任务状态机白名单匹配：
/// - Pending 可进入 Running / Cancelling / Pausing / Succeed / Failed；
/// - Running 可进入 Succeed / Reverting / Failed / Cancelling / Pausing；
/// - Reverting → Reverted、Cancelling → Reverting、Pausing → Paused、
///   Paused → Resuming、Resuming → Running。
/// 未列出的源状态一律返回 `false`。
pub fn verify_task_state_transform(from: TaskState, to: TaskState) -> bool {
    // 幂等：状态未变化时允许（例如重复刷新元数据）。
    if from == to {
        return true;
    }
    match from {
        TASK_STATE_PENDING => matches!(
            to,
            TASK_STATE_RUNNING
                | TASK_STATE_CANCELLING
                | TASK_STATE_PAUSING
                | TASK_STATE_SUCCEED
                | TASK_STATE_FAILED
        ),
        TASK_STATE_RUNNING => matches!(
            to,
            TASK_STATE_SUCCEED
                | TASK_STATE_REVERTING
                | TASK_STATE_FAILED
                | TASK_STATE_CANCELLING
                | TASK_STATE_PAUSING
        ),
        TASK_STATE_REVERTING => to == TASK_STATE_REVERTED,
        TASK_STATE_CANCELLING => to == TASK_STATE_REVERTING,
        TASK_STATE_PAUSING => to == TASK_STATE_PAUSED,
        TASK_STATE_PAUSED => to == TASK_STATE_RESUMING,
        TASK_STATE_RESUMING => to == TASK_STATE_RUNNING,
        // 终态或未知状态禁止继续迁移。
        _ => false,
    }
}

/// Go 风格导出别名，行为与 [`verify_task_state_transform`] 相同。
pub fn VerifyTaskStateTransform(from: TaskState, to: TaskState) -> bool {
    verify_task_state_transform(from, to)
}

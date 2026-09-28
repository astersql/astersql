// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 执行任务类型（TaskType）：描述物理算子落在哪一层执行。
//
// Root 在 TiDB/SQL 层；Cop 在 TiKV 协处理器；MPP 在 TiFlash 等 MPP 节点。
// 物理属性携带 TaskType，决定子任务枚举与是否需要 Exchange。

use std::fmt;

/// TaskType is the type of execution task. The newtype preserves Go's unknown-value fallback.
/// 执行任务类型；newtype 保留 Go 对未知枚举值回退为 "UnknownTaskType" 的语义。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct TaskType(pub i32);

/// 在 TiDB/SQL 层执行的根任务。
pub const RootTaskType: TaskType = TaskType(0);
/// 协处理器单读任务（TableScan / IndexScan）。
pub const CopSingleReadTaskType: TaskType = TaskType(1);
/// 协处理器多读任务（IndexLookup：先索引后回表）。
pub const CopMultiReadTaskType: TaskType = TaskType(2);
/// MPP 任务，当前主要跑在 TiFlash 节点。
pub const MppTaskType: TaskType = TaskType(3);

impl TaskType {
    /// 与 `RootTaskType` 同义的关联常量。
    pub const RootTask: Self = RootTaskType;
    /// 与 `CopSingleReadTaskType` 同义的关联常量。
    pub const CopSingleReadTask: Self = CopSingleReadTaskType;
    /// 与 `CopMultiReadTaskType` 同义的关联常量。
    pub const CopMultiReadTask: Self = CopMultiReadTaskType;
    /// 与 `MppTaskType` 同义的关联常量。
    pub const MppTask: Self = MppTaskType;

    /// 返回任务类型的稳定字符串名；未知值回退为 `UnknownTaskType`。
    pub fn String(self) -> &'static str {
        match self {
            RootTaskType => "rootTask",
            CopSingleReadTaskType => "copSingleReadTask",
            CopMultiReadTaskType => "copMultiReadTask",
            MppTaskType => "mppTask",
            _ => "UnknownTaskType",
        }
    }
}

impl fmt::Display for TaskType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.String())
    }
}

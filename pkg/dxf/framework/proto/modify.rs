// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 任务运行时修改协议。
//
// 调度器可在任务执行中调整 required slots、最大节点数、加索引 batch size
// 或写速度上限。ModificationType 常量字符串需与持久化/兼容层保持一致
//（例如 required slots 仍使用历史名 `modify_concurrency`）。

use super::task::TaskState;

// ModificationType is the type of task modification.
// ModificationType 对应 Go 的 string 别名，用于区分不同修改类型。
pub type ModificationType = &'static str;

// ModificationTypeExt 对应 Go 的 fmt.Stringer 实现。
pub trait ModificationTypeExt {
    /// 返回修改类型的字符串形式。
    fn String(&self) -> String;
}

impl ModificationTypeExt for ModificationType {
    // String implements fmt.Stringer interface.
    fn String(&self) -> String {
        (*self).to_string()
    }
}

// ModifyRequiredSlots is the type for modifying task required slots.
// Note: required slots is introduced later and separated from the old
// "concurrency" concept, we still use "modify_concurrency" as the modification
// type for compatibility.
// 修改任务所需 slot（并发度）。
// 注意：required slots 是后期引入、与旧 concurrency 概念分离的，
// 为兼容仍使用 "modify_concurrency" 作为修改类型名。
pub const ModifyRequiredSlots: ModificationType = "modify_concurrency";
// ModifyMaxNodeCount is the type for modifying max node count of task.
// 修改任务最大节点数。
pub const ModifyMaxNodeCount: ModificationType = "modify_max_node_count";
// ModifyBatchSize is the type for modifying batch size of add-index.
// 修改加索引（add-index）的 batch size。
pub const ModifyBatchSize: ModificationType = "modify_batch_size";
// ModifyMaxWriteSpeed is the type for modifying max write speed of add-index.
// 修改加索引的最大写入速度。
pub const ModifyMaxWriteSpeed: ModificationType = "modify_max_write_speed";

// ModifyParam is the parameter for task modification.
// ModifyParam 对应 Go 结构体，json tag 仅在注释中保留，后续接 serde 时再映射。
pub struct ModifyParam {
    // json: "prev_state"
    pub PrevState: TaskState,
    // json: "modifications"
    pub Modifications: Vec<Modification>,
}

/// 将 PrevState 与全部 Modification 格式化为可读字符串。
impl ModifyParam {
    // String implements fmt.Stringer interface.
    pub fn String(&self) -> String {
        let modifications = self
            .Modifications
            .iter()
            .map(Modification::String)
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{{prev_state: {}, modifications: [{}]}}",
            self.PrevState, modifications
        )
    }
}

// Modification is one modification for task.
// Modification 对应单条修改请求，To 保留 Go 中 int64 目标值语义。
#[derive(Debug)]
pub struct Modification {
    // json: "type"
    pub Type: ModificationType,
    // json: "to"
    pub To: i64,
}

/// 单条修改的 `{type, to}` 展示。
impl Modification {
    // String implements fmt.Stringer interface.
    pub fn String(&self) -> String {
        format!("{{type: {}, to: {}}}", self.Type, self.To)
    }
}

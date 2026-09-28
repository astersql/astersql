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
// DXF 内置任务类型常量及与整型编码的双向转换。
//
// 整型编码用于需要紧凑表示的场景；未知类型往返为 0 / 空字符串。

// limitations under the License.

use super::task::TaskType;

// TaskTypeExample is TaskType of Example, it's for test.
/// 示例/测试用任务类型。
pub const TaskTypeExample: TaskType = "Example";
// ImportInto is TaskType of ImportInto.
/// IMPORT INTO 导入任务。
pub const ImportInto: TaskType = "ImportInto";
// Backfill is TaskType of add index Backfilling process.
/// 加索引 backfill（回填）过程任务。
pub const Backfill: TaskType = "backfill";

// Type2Int converts task type to int.
/// 任务类型 → 整型编码；未知返回 0。
pub fn Type2Int(t: TaskType) -> i32 {
    match t {
        TaskTypeExample => 1,
        ImportInto => 2,
        Backfill => 3,
        // Go 默认返回 0；保留未知类型的兜底编码。
        _ => 0,
    }
}

// Int2Type converts int to task type.
/// 整型编码 → 任务类型；未知返回空串。
pub fn Int2Type(i: i32) -> TaskType {
    match i {
        1 => TaskTypeExample,
        2 => ImportInto,
        3 => Backfill,
        // Go 默认返回空 TaskType；这里用空字符串表达相同语义。
        _ => "",
    }
}

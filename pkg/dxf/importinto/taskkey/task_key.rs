// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Import Into task-key construction, migrated from `task_key.go`.
//
// 本模块负责构造 Import Into（导入任务）在 DXF（分布式执行框架）中的 task key。
// task key 是用于唯一标识一次导入作业的路径字符串，供调度与元数据检索。
// NextGen 内核下会附加 keyspace（键空间，多租户隔离域）前缀；
// classic 模式保持旧格式，仅含任务类型与 job ID。

#![allow(dead_code, non_snake_case)]

use crate::{kerneltype, keyspace, proto};

/// 根据当前 kernel 类型返回 import-into job 的 task key。
///
/// 对应 Go 的 `ForJob`。NextGen 走带 keyspace 的路径；classic 返回 `ImportInto/{jobID}`。
// ForJob 对应 Go 的 ForJob：根据当前 kernel 类型返回 import-into job 的 task key。
pub fn ForJob(jobID: i64) -> String {
    if kerneltype::IsNextGen() {
        // NextGen 模式下 task key 带 keyspace 前缀，keyspace 来自全局配置。
        return forJobInKeyspace(keyspace::GetKeyspaceNameBySettings(), jobID);
    }

    // classic 模式保持旧格式，只包含 task type 和 job ID。
    format!("{}/{}", proto::ImportInto, jobID)
}

/// 显式为指定 keyspace 生成 import-into job 的 task key。
///
/// 对应 Go 的 `ForJobInKeyspace`。classic 模式下忽略 `keyspaceName`，
/// 因为旧格式 task key 本身不按 keyspace 分域。
// ForJobInKeyspace 对应 Go 的 ForJobInKeyspace：显式为某个 keyspace 生成 task key。
// classic 模式下 Go 会忽略 keyspaceName，因为 task key 本身不按 keyspace 分域。
pub fn ForJobInKeyspace(keyspaceName: String, jobID: i64) -> String {
    if kerneltype::IsNextGen() {
        return forJobInKeyspace(keyspaceName, jobID);
    }

    ForJob(jobID)
}

/// 私有辅助：按 `keyspace/ImportInto/jobID` 顺序拼接路径。
// forJobInKeyspace 保留 Go 的私有辅助函数：把 keyspace、task type、job ID 顺序拼为路径。
fn forJobInKeyspace(keyspaceName: String, jobID: i64) -> String {
    format!("{}/{}/{}", keyspaceName, proto::ImportInto, jobID)
}

// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Optimizer Trace（优化器跟踪）目录命名。
//
// 提供外部存储上 optimizer trace 相对路径的构造：目录固定为
// `optimizer_trace/<instance_id>`。instance_id 优先取已注册的 server ID，
// 否则回退到当前进程 PID，对齐 Go `GetOptimizerTraceDirName` 的兜底语义。

// GetOptimizerTraceDirName 对应 Go 的同名函数：返回 optimizer trace 在外部存储中的相对目录。
// pub fn GetOptimizerTraceDirName() -> String {
//     let mut instanceID = String::new();
//     if let Ok(info) = infosync::GetServerInfo() {
// Go 还判断 info != nil；用 Option 形状表达这个空指针分支。
//         if let Some(info) = info {
//             instanceID = info.ID;
//         }
//     }
//
// instanceID 为空时退回到当前进程 ID，保留 Go 中 “server info 未设置” 的兜底语义。
//     if instanceID.is_empty() {
//         instanceID = os::Getpid().to_string();
//     }
//
//     filepath::Join("optimizer_trace", &instanceID)
// }
// */
use std::path::PathBuf;

use astersql_domain_infosync as infosync;

/// Optimizer Trace 在外部存储中的根目录名。
pub const OPTIMIZER_TRACE_DIR: &str = "optimizer_trace";

fn optimizer_trace_dir_for_server_id(server_id: Option<&str>) -> PathBuf {
    let instance = server_id
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| std::process::id().to_string());
    PathBuf::from(OPTIMIZER_TRACE_DIR).join(instance)
}

/// Returns the relative optimizer-trace directory for external storage.
///
/// Like Go `GetOptimizerTraceDirName`, this owns the global server-info lookup.
/// A missing syncer, lookup error, or empty server ID falls back to the process ID.
pub fn get_optimizer_trace_dir_name() -> PathBuf {
    let server_info = infosync::GetServerInfo().ok();
    optimizer_trace_dir_for_server_id(server_info.as_ref().map(|info| info.ID.as_str()))
}

#[cfg(test)]
pub(crate) fn optimizer_trace_dir_for_server_id_for_test(server_id: Option<&str>) -> PathBuf {
    optimizer_trace_dir_for_server_id(server_id)
}

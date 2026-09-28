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

// 分布式任务（disttask）执行器 ID 生成与匹配。
//
// 对应 Go `idservice`：把 InfoSync 中的节点信息格式化为 `host:port`（IPv6 加方括号），
// 供调度器定位子任务执行节点。InfoSync 负责集群内 TiDB 实例的元信息同步。

use infosync::ServerInfo;

/// Generates the distributed-task executor ID in Go's `net.JoinHostPort` shape.
/// 按 Go `net.JoinHostPort` 形态生成执行器 ID（`IP:Port` / `[IPv6]:Port`）。
pub fn GenerateExecID(info: &ServerInfo) -> String {
    join_host_port(&info.IP, info.Port)
}

/// 判断 `schedulerID` 是否能在 `serverInfos` 中匹配到某节点的执行器 ID。
pub fn MatchServerInfo(serverInfos: &[ServerInfo], schedulerID: &str) -> bool {
    FindServerInfo(serverInfos, schedulerID) >= 0
}

/// 在列表中按执行器 ID 查找下标；未找到返回 -1（与 Go 一致）。
pub fn FindServerInfo(serverInfos: &[ServerInfo], schedulerID: &str) -> isize {
    serverInfos
        .iter()
        .position(|server| GenerateExecID(server) == schedulerID)
        .map_or(-1, |index| index as isize)
}

/// Resolves an executor ID through the current InfoSync server registry.
/// 经当前 InfoSync 注册表用节点 id 解析执行器 ID；缺失则返回空串。
pub fn GenerateSubtaskExecID(id: &str) -> String {
    infosync::GetAllServerInfo()
        .ok()
        .and_then(|servers| servers.get(id).map(GenerateExecID))
        .unwrap_or_default()
}

/// Resolves an executor ID through the test server registry.
/// 测试用：经 Mock InfoSync 注册表解析执行器 ID。
pub fn GenerateSubtaskExecID4Test(id: &str) -> String {
    infosync::MockGlobalServerInfoManagerEntry()
        .GetAllServerInfo()
        .get(id)
        .map(GenerateExecID)
        .unwrap_or_default()
}

/// 拼接 host:port；host 含 `:` 时视为 IPv6 并加方括号。
fn join_host_port(host: &str, port: u32) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

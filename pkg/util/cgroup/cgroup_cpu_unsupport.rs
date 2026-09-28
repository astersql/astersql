// Copyright 2022 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 非 Linux（或不支持 cgroup CPU）平台的 CPU 查询桩实现。
//
// 无法读取容器配额时，退化为主机并行度，并声明 CPU 配额未定义。

#![allow(non_snake_case)]
use super::*;
use anyhow::Result;

/// 返回仅填充 `NumCPU`（主机可用并行度）的用法快照，其余字段为默认值。
pub fn GetCgroupCPU() -> Result<CPUUsage> {
    Ok(CPUUsage {
        NumCPU: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1) as i32,
        ..CPUUsage::default()
    })
}

/// 不支持平台上返回 `(-1, -1)`，表示无 period/quota。
pub fn GetCPUPeriodAndQuota() -> Result<(i64, i64)> {
    Ok((-1, -1))
}
/// 无法推导 GOMAXPROCS 时返回 `-1` 与 `CPUQuotaUndefined`。
pub fn CPUQuotaToGOMAXPROCS(_: i32) -> Result<(i32, CPUQuotaStatus)> {
    Ok((-1, CPUQuotaStatus::CPUQuotaUndefined))
}
/// 不支持平台一律视为非容器环境。
pub fn InContainer() -> bool {
    false
}

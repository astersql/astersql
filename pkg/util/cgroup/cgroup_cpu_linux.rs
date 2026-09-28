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

// Linux 平台 cgroup CPU 公开入口。
//
// 基于根文件系统 `/` 读取本进程所属 cgroup 的 CPU 配额与用量，
// 并将配额换算为近似 GOMAXPROCS；同时提供粗粒度“是否在容器内”探测。

#![allow(non_snake_case)]
use super::*;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// 读取本机 cgroup CPU 用法，并填入主机可用并行度作为 `NumCPU`。
pub fn GetCgroupCPU() -> Result<CPUUsage> {
    let mut usage = getCgroupCPU(Path::new("/"))?;
    usage.NumCPU = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1) as i32;
    Ok(usage)
}

/// 将 cgroup CPU shares（quota/period 向上取整）映射为建议并行度。
///
/// 若结果低于 `minValue` 则钳制到下限并标记 `CPUQuotaMinUsed`。
pub fn CPUQuotaToGOMAXPROCS(minValue: i32) -> Result<(i32, CPUQuotaStatus)> {
    let max = GetCgroupCPU()?.CPUShares().ceil() as i32;
    if minValue > 0 && max < minValue {
        Ok((minValue, CPUQuotaStatus::CPUQuotaMinUsed))
    } else {
        Ok((max, CPUQuotaStatus::CPUQuotaUsed))
    }
}

/// 仅返回本机 cgroup 的 CPU period 与 quota（微秒）。
pub fn GetCPUPeriodAndQuota() -> Result<(i64, i64)> {
    getCgroupCPUPeriodAndQuota(Path::new("/"))
}
/// 通过 `/proc/self/cgroup` 或 `mountinfo` 粗判是否运行在容器中。
pub fn InContainer() -> bool {
    inContainer(Path::new(procPathCGroup)) || inContainer(Path::new(procPathMountInfo))
}

/// 检查单个 proc 文件：cgroup 路径含 docker/kubepods/containerd，或根挂载为 overlay。
pub(crate) fn inContainer(path: &Path) -> bool {
    let Ok(content) = fs::read(path) else {
        return false;
    };
    in_container_content(path, &content)
}

pub(crate) fn in_container_content(path: &Path, content: &[u8]) -> bool {
    if path == Path::new(procPathCGroup)
        && [b"docker".as_slice(), b"kubepods", b"containerd"]
            .iter()
            .any(|needle| {
                content
                    .windows(needle.len())
                    .any(|window| window == *needle)
            })
    {
        return true;
    }
    path == Path::new(procPathMountInfo)
        && content.split(|byte| *byte == b'\n').any(|line| {
            let fields: Vec<_> = line.split(|byte| *byte == b' ').collect();
            fields.len() > 8 && fields[4] == b"/" && fields[8] == b"overlay"
        })
}

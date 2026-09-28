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

// 在给定 root（可指向临时目录 mock）下解析 cgroup CPU 配额与用量。
//
// 同时支持 v1（cpu/cpuacct）与 v2（unified），混合挂载时优先尝试 v2 再回退 v1。

#![allow(non_snake_case)]
use super::*;
use anyhow::{Result, anyhow};
use std::path::Path;

/// 探测 cpu,cpuacct 控制器，读取 period/quota 与 user/sys 用量。
pub(crate) fn getCgroupCPU(root: &Path) -> Result<CPUUsage> {
    let path = detectControlPath(&join_root(root, procPathCGroup), "cpu,cpuacct")?;
    if path.is_empty() {
        return Err(anyhow!("no cpu controller detected"));
    }
    let (mounts, versions) =
        getCgroupDetails(&join_root(root, procPathMountInfo), &path, "cpu,cpuacct")?;
    let mut result = CPUUsage::default();
    // 同时存在 v1 与 v2 挂载时优先读 v2，失败再回退 v1。
    let ((period, quota), (stime, utime)) = if mounts.len() == 2 {
        let v1 = join_root(root, &mounts[0]);
        let v2 = join_root(join_root(root, &mounts[1]), &path);
        (
            detectCPUQuotaInV2(&v2).or_else(|_| detectCPUQuotaInV1(&v1))?,
            detectCPUUsageInV2(&v2).or_else(|_| detectCPUUsageInV1(&v1))?,
        )
    } else {
        let cgroot = join_root(
            join_root(root, &mounts[0]),
            if versions[0] == 2 { &path } else { "" },
        );
        match versions[0] {
            1 => (detectCPUQuotaInV1(&cgroot)?, detectCPUUsageInV1(&cgroot)?),
            2 => (detectCPUQuotaInV2(&cgroot)?, detectCPUUsageInV2(&cgroot)?),
            version => return Err(anyhow!("detected unknown cgroup version index: {version}")),
        }
    };
    result.Period = period;
    result.Quota = quota;
    result.Stime = stime;
    result.Utime = utime;
    Ok(result)
}

/// 仅探测 cpu 控制器并返回 (period, quota)。
pub(crate) fn getCgroupCPUPeriodAndQuota(root: &Path) -> Result<(i64, i64)> {
    let path = detectControlPath(&join_root(root, procPathCGroup), "cpu")?;
    if path.is_empty() {
        return Err(anyhow!("no cpu controller detected"));
    }
    let (mounts, versions) = getCgroupDetails(&join_root(root, procPathMountInfo), &path, "cpu")?;
    if mounts.len() == 2 {
        let v1 = join_root(root, &mounts[0]);
        let v2 = join_root(join_root(root, &mounts[1]), &path);
        return detectCPUQuotaInV2(&v2).or_else(|_| detectCPUQuotaInV1(&v1));
    }
    let cgroot = join_root(
        join_root(root, &mounts[0]),
        if versions[0] == 2 { &path } else { "" },
    );
    match versions[0] {
        1 => detectCPUQuotaInV1(&cgroot),
        2 => detectCPUQuotaInV2(&cgroot),
        version => Err(anyhow!("detected unknown cgroup version index: {version}")),
    }
}

impl CPUUsage {
    /// 有效 CPU 份额：有正 period/quota 时为 quota/period，否则退回 `NumCPU`。
    pub fn CPUShares(&self) -> f64 {
        if self.Period <= 0 || self.Quota <= 0 {
            self.NumCPU as f64
        } else {
            self.Quota as f64 / self.Period as f64
        }
    }
}

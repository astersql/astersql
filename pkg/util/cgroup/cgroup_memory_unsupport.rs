// Copyright 2025 PingCAP, Inc.
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

// 不支持平台对外的内存查询桩，以及可注入 root 的探测辅助（供测试/跨平台复用）。
//
// 公开 `Get*` 恒返回 0/`Unknown`；内部 `getCgroup*` / `detect*` 仍可按 v1/v2
// 路径读取（与 Linux 版语义对齐，便于 mock 测）。

#![allow(non_snake_case)]
use super::*;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// 不支持平台：内存限额恒为 0。
pub fn GetMemoryLimit() -> Result<u64> {
    Ok(0)
}
/// 不支持平台：限额 0，版本 `Unknown`。
pub fn GetCgroupMemLimit() -> Result<(u64, Version)> {
    Ok((0, Unknown))
}
/// 不支持平台：用量恒为 0。
pub fn GetMemoryUsage() -> Result<u64> {
    Ok(0)
}
/// 不支持平台：inactive file 用量恒为 0。
pub fn GetMemoryInactiveFileUsage() -> Result<u64> {
    Ok(0)
}

/// 解析 `root` 下 memory controller 的控制路径、挂载点与 cgroup 版本。
fn location(root: &Path) -> Result<Option<(String, Vec<String>, Vec<i32>)>> {
    let path = detectControlPath(&join_root(root, procPathCGroup), "memory")?;
    if path.is_empty() {
        return Ok(None);
    }
    let (mounts, versions) =
        getCgroupDetails(&join_root(root, procPathMountInfo), &path, "memory")?;
    Ok(Some((path, mounts, versions)))
}

/// 在给定 root 下读取当前 cgroup 内存用量（v1 `usage_in_bytes` / v2 `memory.current`）。
pub fn getCgroupMemUsage(root: &Path) -> Result<u64> {
    let Some((path, mounts, versions)) = location(root)? else {
        return Ok(0);
    };
    if versions.len() == 2 {
        return detectMemUsageInV1(&join_root(root, &mounts[0]))
            .or_else(|_| detectMemUsageInV2(&join_root(join_root(root, &mounts[0]), &path)));
    }
    let cgroot = join_root(
        join_root(root, &mounts[0]),
        if versions[0] == 2 { &path } else { "" },
    );
    match versions[0] {
        1 => detectMemUsageInV1(&cgroot),
        2 => detectMemUsageInV2(&cgroot),
        version => Err(anyhow!("detected unknown cgroup version index: {version}")),
    }
}

/// 读取 memory.stat 中的 inactive file 占用（可回收页缓存相关）。
pub fn getCgroupMemInactiveFileUsage(root: &Path) -> Result<u64> {
    let Some((path, mounts, versions)) = location(root)? else {
        return Ok(0);
    };
    if versions.len() == 2 {
        return detectMemInactiveFileInV1(&join_root(root, &mounts[0])).or_else(|_| {
            detectMemInactiveFileInV2(&join_root(join_root(root, &mounts[0]), &path))
        });
    }
    let cgroot = join_root(
        join_root(root, &mounts[0]),
        if versions[0] == 2 { &path } else { "" },
    );
    match versions[0] {
        1 => detectMemInactiveFileInV1(&cgroot),
        2 => detectMemInactiveFileInV2(&cgroot),
        version => Err(anyhow!("detected unknown cgroup version index: {version}")),
    }
}

/// 读取 cgroup 内存硬限制，并标注探测到的版本（V1/V2）。
pub fn getCgroupMemLimit(root: &Path) -> Result<(u64, Version)> {
    let Some((path, mounts, versions)) = location(root)? else {
        return Ok((0, Unknown));
    };
    if versions.len() == 2 {
        return detectMemLimitInV1(&join_root(root, &mounts[0]))
            .or_else(|_| detectMemLimitInV2(&join_root(join_root(root, &mounts[0]), &path)));
    }
    let cgroot = join_root(
        join_root(root, &mounts[0]),
        if versions[0] == 2 { &path } else { "" },
    );
    match versions[0] {
        1 => detectMemLimitInV1(&cgroot),
        2 => detectMemLimitInV2(&cgroot),
        version => Err(anyhow!("detected unknown cgroup version index: {version}")),
    }
}

/// cgroup v1：从 `memory.usage_in_bytes` 读用量。
pub fn detectMemUsageInV1(root: &Path) -> Result<u64> {
    readInt64Value(root, cgroupV1MemUsage, 1)
}
/// cgroup v2：从 `memory.current` 读用量。
pub fn detectMemUsageInV2(root: &Path) -> Result<u64> {
    readInt64Value(root, cgroupV2MemUsage, 2)
}
/// cgroup v1：从 memory.stat 的 hierarchical_memory_limit 读限额。
pub fn detectMemLimitInV1(root: &Path) -> Result<(u64, Version)> {
    Ok((
        detectMemStatValue(root, cgroupV1MemStat, cgroupV1MemLimitStatKey, 1)?,
        V1,
    ))
}
/// cgroup v2：从 `memory.max` 读限额。
pub fn detectMemLimitInV2(root: &Path) -> Result<(u64, Version)> {
    Ok((readInt64Value(root, cgroupV2MemLimit, 2)?, V2))
}
/// cgroup v1：读 `total_inactive_file`。
pub fn detectMemInactiveFileInV1(root: &Path) -> Result<u64> {
    detectMemStatValue(
        root,
        cgroupV1MemStat,
        cgroupV1MemInactiveFileUsageStatKey,
        1,
    )
}
/// cgroup v2：读 `inactive_file`。
pub fn detectMemInactiveFileInV2(root: &Path) -> Result<u64> {
    detectMemStatValue(
        root,
        cgroupV2MemStat,
        cgroupV2MemInactiveFileUsageStatKey,
        2,
    )
}
/// 在 memory.stat 风格文件中查找键并解析为 u64。
pub fn detectMemStatValue(root: &Path, filename: &str, key: &str, version: i32) -> Result<u64> {
    let file = fs::File::open(root.join(filename))
        .with_context(|| format!("can't read file {filename} from cgroup v{version}"))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        // Go Scanner stops at a 64 KiB token and detectMemStatValue deliberately
        // ignores its terminal error. Preserve that missing-key result here.
        let read = reader.by_ref().take(65536).read_until(b'\n', &mut bytes);
        if !matches!(read, Ok(n) if n > 0) || (bytes.len() == 65536 && bytes.last() != Some(&b'\n'))
        {
            break;
        }
        let line = String::from_utf8_lossy(&bytes);
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() == 2 && fields[0] == key {
            if !fields[1].bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(anyhow!(
                    "can't read {key:?} memory stat from cgroup v{version} in {filename}"
                ));
            }
            return fields[1].parse().with_context(|| {
                format!("can't read {key:?} memory stat from cgroup v{version} in {filename}")
            });
        }
    }
    Err(anyhow!(
        "failed to find expected memory stat {key:?} for cgroup v{version} in {filename}"
    ))
}

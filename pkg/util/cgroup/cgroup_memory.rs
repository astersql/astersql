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

// Linux cgroup 内存限额与用量查询。
//
// 对外 `Get*` 以主机根 `/` 为基准；内部 `getCgroup*` 可注入 root 以便测试。
// 同时存在 v1/v2 挂载时优先尝试一侧并在失败时回退。

#![allow(non_snake_case)]
use super::*;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// 返回本机 cgroup 内存硬限制（字节）。
pub fn GetMemoryLimit() -> Result<u64> {
    Ok(getCgroupMemLimit(Path::new("/"))?.0)
}
/// 返回本机内存硬限制及探测到的 cgroup 版本。
pub fn GetCgroupMemLimit() -> Result<(u64, Version)> {
    getCgroupMemLimit(Path::new("/"))
}
/// 返回本机 cgroup 当前内存用量。
pub fn GetMemoryUsage() -> Result<u64> {
    getCgroupMemUsage(Path::new("/"))
}
/// 返回 inactive file 页占用（可回收缓存相关）。
pub fn GetMemoryInactiveFileUsage() -> Result<u64> {
    getCgroupMemInactiveFileUsage(Path::new("/"))
}

/// 定位 memory 控制器路径、挂载点与版本；无控制器时返回 `None`。
fn memory_location(root: &Path) -> Result<Option<(String, Vec<String>, Vec<i32>)>> {
    let path = detectControlPath(&join_root(root, procPathCGroup), "memory")?;
    if path.is_empty() {
        eprintln!("WARN no cgroup memory controller detected");
        return Ok(None);
    }
    let (mounts, versions) =
        getCgroupDetails(&join_root(root, procPathMountInfo), &path, "memory")?;
    Ok(Some((path, mounts, versions)))
}

/// 在给定 root 下读取 inactive file 用量。
pub fn getCgroupMemInactiveFileUsage(root: &Path) -> Result<u64> {
    let Some((path, mounts, versions)) = memory_location(root)? else {
        return Ok(0);
    };
    if mounts.len() == 2 {
        return detectMemInactiveFileUsageInV1(&join_root(root, &mounts[0])).or_else(|_| {
            detectMemInactiveFileUsageInV2(&join_root(join_root(root, &mounts[1]), &path))
        });
    }
    let cgroot = join_root(
        join_root(root, &mounts[0]),
        if versions[0] == 2 { &path } else { "" },
    );
    match versions[0] {
        1 => detectMemInactiveFileUsageInV1(&cgroot),
        2 => detectMemInactiveFileUsageInV2(&cgroot),
        version => Err(anyhow!("detected unknown cgroup version index: {version}")),
    }
}

/// 在给定 root 下读取当前内存用量。
pub fn getCgroupMemUsage(root: &Path) -> Result<u64> {
    let Some((path, mounts, versions)) = memory_location(root)? else {
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

/// 在给定 root 下读取内存硬限制及版本。
pub fn getCgroupMemLimit(root: &Path) -> Result<(u64, Version)> {
    let Some((path, mounts, versions)) = memory_location(root)? else {
        return Ok((0, Unknown));
    };
    if versions.len() == 2 {
        return match detectMemLimitInV1(&join_root(root, &mounts[0])) {
            Ok(limit) => Ok((limit, V1)),
            Err(_) => Ok((
                detectMemLimitInV2(&join_root(join_root(root, &mounts[1]), &path))?,
                V2,
            )),
        };
    }
    let cgroot = join_root(
        join_root(root, &mounts[0]),
        if versions[0] == 2 { &path } else { "" },
    );
    match versions[0] {
        1 => Ok((detectMemLimitInV1(&cgroot)?, V1)),
        2 => Ok((detectMemLimitInV2(&cgroot)?, V2)),
        version => Err(anyhow!("detected unknown cgroup version index: {version}")),
    }
}

/// v1：从 memory.stat 的 hierarchical_memory_limit 读限额。
pub fn detectMemLimitInV1(root: &Path) -> Result<u64> {
    detectMemStatValue(root, cgroupV1MemStat, cgroupV1MemLimitStatKey, 1)
}
/// v2：从 `memory.max` 读限额。
pub fn detectMemLimitInV2(root: &Path) -> Result<u64> {
    readInt64Value(root, cgroupV2MemLimit, 2)
}
/// v1：读 `total_inactive_file`。
pub fn detectMemInactiveFileUsageInV1(root: &Path) -> Result<u64> {
    detectMemStatValue(
        root,
        cgroupV1MemStat,
        cgroupV1MemInactiveFileUsageStatKey,
        1,
    )
}
/// v2：读 `inactive_file`。
pub fn detectMemInactiveFileUsageInV2(root: &Path) -> Result<u64> {
    detectMemStatValue(
        root,
        cgroupV2MemStat,
        cgroupV2MemInactiveFileUsageStatKey,
        2,
    )
}
/// v1：读 `memory.usage_in_bytes`。
pub fn detectMemUsageInV1(root: &Path) -> Result<u64> {
    readInt64Value(root, cgroupV1MemUsage, 1)
}
/// v2：读 `memory.current`。
pub fn detectMemUsageInV2(root: &Path) -> Result<u64> {
    readInt64Value(root, cgroupV2MemUsage, 2)
}

/// 在 memory.stat 中查找键并解析数值。
pub fn detectMemStatValue(root: &Path, filename: &str, key: &str, version: i32) -> Result<u64> {
    let path = root.join(filename);
    let file = fs::File::open(&path)
        .with_context(|| format!("can't read file {filename} from cgroup v{version}"))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        // Go Scanner stops at a 64 KiB token and its error is deliberately
        // ignored by detectMemStatValue. Limit allocation and preserve that
        // missing-key result, including read errors and non-UTF-8 input.
        let read = reader.by_ref().take(65536).read_until(b'\n', &mut bytes);
        if !matches!(read, Ok(n) if n > 0) || (bytes.len() == 65536 && bytes.last() != Some(&b'\n'))
        {
            break;
        }
        let line = String::from_utf8_lossy(&bytes);
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 2 || fields[0] != key {
            continue;
        }
        // strconv.ParseUint with base 10 rejects a leading plus sign.
        if !fields[1].bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(anyhow!(
                "can't read {key:?} memory stat from cgroup v{version} in {filename}"
            ));
        }
        return fields[1].parse().with_context(|| {
            format!("can't read {key:?} memory stat from cgroup v{version} in {filename}")
        });
    }
    Err(anyhow!(
        "failed to find expected memory stat {key:?} for cgroup v{version} in {filename}"
    ))
}

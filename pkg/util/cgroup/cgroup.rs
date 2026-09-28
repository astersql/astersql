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

// cgroup 基础设施：路径常量、版本探测、CPU 配额/用量文件解析。
//
// 控制组（cgroup）是 Linux 用来限制进程资源的机制。本模块从 `/proc/self/cgroup`
// 与 `mountinfo` 定位控制器挂载点，并解析 v1/v2 的 CPU 相关接口文件。

#![allow(non_snake_case, non_upper_case_globals)]

use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 将 cgroup CPU 配额映射到并行度时的状态标记。
pub enum CPUQuotaStatus {
    /// 未定义或无法取得有效配额。
    CPUQuotaUndefined = 0,
    /// 使用探测到的配额换算结果。
    CPUQuotaUsed = 1,
    /// 配额低于下限，已钳制到 `minValue`。
    CPUQuotaMinUsed = 2,
}

/// cgroup v1/v2 memory.stat 文件名（二者同名）。
pub(crate) const cgroupV1MemStat: &str = "memory.stat";
pub(crate) const cgroupV2MemStat: &str = "memory.stat";
/// cgroup v2 内存硬限制文件。
pub(crate) const cgroupV2MemLimit: &str = "memory.max";
/// cgroup v1 当前内存用量文件。
pub(crate) const cgroupV1MemUsage: &str = "memory.usage_in_bytes";
/// cgroup v2 当前内存用量文件。
pub(crate) const cgroupV2MemUsage: &str = "memory.current";
/// cgroup v1 CFS CPU 配额（微秒）。
const cgroupV1CPUQuota: &str = "cpu.cfs_quota_us";
/// cgroup v1 CFS 调度周期（微秒）。
const cgroupV1CPUPeriod: &str = "cpu.cfs_period_us";
/// cgroup v1 内核态 CPU 累计用量。
const cgroupV1CPUSysUsage: &str = "cpuacct.usage_sys";
/// cgroup v1 用户态 CPU 累计用量。
const cgroupV1CPUUserUsage: &str = "cpuacct.usage_user";
/// cgroup v2 CPU 配额文件（`quota period` 或 `max`）。
const cgroupV2CPUMax: &str = "cpu.max";
/// cgroup v2 CPU 统计文件（含 user_usec/system_usec）。
const cgroupV2CPUStat: &str = "cpu.stat";
/// v1 memory.stat 中 inactive file 键名。
pub(crate) const cgroupV1MemInactiveFileUsageStatKey: &str = "total_inactive_file";
/// v2 memory.stat 中 inactive file 键名。
pub(crate) const cgroupV2MemInactiveFileUsageStatKey: &str = "inactive_file";
/// v1 hierarchical 内存限制键名。
pub(crate) const cgroupV1MemLimitStatKey: &str = "hierarchical_memory_limit";
/// 本进程所属 cgroup 层次信息。
pub(crate) const procPathCGroup: &str = "/proc/self/cgroup";
/// 本进程可见的挂载信息（用于定位 cgroup 挂载点）。
pub(crate) const procPathMountInfo: &str = "/proc/self/mountinfo";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 某次 cgroup CPU 采样：内核/用户时间、周期、配额与可见 CPU 数。
pub struct CPUUsage {
    /// 内核态累计 CPU 时间（v1 纳秒 / v2 微秒，按文件语义）。
    pub Stime: u64,
    /// 用户态累计 CPU 时间。
    pub Utime: u64,
    /// CFS/CPU.max 周期。
    pub Period: i64,
    /// 周期内可用配额；`-1`/`max` 表示不限制。
    pub Quota: i64,
    /// 主机或容器可见的逻辑 CPU 数。
    pub NumCPU: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 探测到的 cgroup 协议版本。
pub enum Version {
    /// 未能判定版本。
    Unknown = 0,
    /// 传统分级 cgroup v1。
    V1 = 1,
    /// 统一层次 cgroup v2。
    V2 = 2,
}
pub use Version::{Unknown, V1, V2};

/// 模块内模拟的“最大并行度”存储（对齐 Go GOMAXPROCS 可观测契约）。
static MAX_PROCS: AtomicUsize = AtomicUsize::new(0);

/// Rust has no process-wide scheduler knob equivalent to Go's GOMAXPROCS.  We
/// retain the observable set/undo contract for migrated callers in this module.
///
/// 按 cgroup CPU 配额设置模块内 `MAX_PROCS`，返回用于恢复旧值的闭包；
/// 若环境变量 `GOMAXPROCS` 已存在或配额未定义则不做改动。
pub fn SetGOMAXPROCS() -> Result<Box<dyn FnOnce() + Send>> {
    if std::env::var_os("GOMAXPROCS").is_some() {
        return Ok(Box::new(|| {}));
    }
    let (max_procs, status) = super::CPUQuotaToGOMAXPROCS(1)?;
    if status == CPUQuotaStatus::CPUQuotaUndefined {
        return Ok(Box::new(|| {}));
    }
    let previous = MAX_PROCS.swap(max_procs as usize, Ordering::SeqCst);
    Ok(Box::new(move || {
        MAX_PROCS.store(previous, Ordering::SeqCst);
    }))
}

/// 将 `child` 的普通路径分量接到 `root` 下（忽略绝对路径前缀等）。
pub(crate) fn join_root(root: impl AsRef<Path>, child: impl AsRef<Path>) -> PathBuf {
    let mut result = root.as_ref().to_path_buf();
    for component in child.as_ref().components() {
        if let Component::Normal(part) = component {
            result.push(part);
        }
    }
    result
}

/// 判断 cgroup 控制器字段是否匹配（支持逗号分隔的多控制器集合包含关系）。
pub(crate) fn controllerMatch(field: &str, controller: &str) -> bool {
    if field == controller {
        return true;
    }
    let fields: HashSet<_> = field.split(',').collect();
    let controllers: Vec<_> = controller.split(',').collect();
    fields.len() >= 2
        && fields.len() >= controllers.len()
        && controllers
            .iter()
            .all(|controller| fields.contains(controller))
}

/// 从 `/proc/.../cgroup` 解析指定控制器的相对控制路径；无匹配时回退到 unified 路径。
pub(crate) fn detectControlPath(path: &Path, controller: &str) -> Result<String> {
    let file = fs::File::open(path).with_context(|| {
        format!(
            "failed to read {controller} cgroup from cgroups file: {}",
            path.display()
        )
    })?;
    let mut unified = String::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<_> = line.split(':').collect();
        if fields.len() < 3 {
            continue;
        }
        if fields[0] == "0" && fields[1].is_empty() {
            unified = fields[2].to_owned();
        } else if controllerMatch(fields[1], controller) {
            return Ok(fields[2..].join(":"));
        }
    }
    Ok(unified)
}

/// 解析 mountinfo 一行：返回 (版本号, 是否匹配该控制器)。
pub(crate) fn detectCgroupVersion(fields: &[&str], controller: &str) -> (i32, bool) {
    if fields.len() < 10 {
        return (0, false);
    }
    // mountinfo 可选字段后以 "-" 分隔，其后为 fs 类型与超级块选项。
    let Some(separator) = fields
        .iter()
        .enumerate()
        .skip(6)
        .find_map(|(i, f)| (*f == "-").then_some(i))
    else {
        return (0, false);
    };
    if fields.len().saturating_sub(separator + 1) < 3 {
        return (0, false);
    }
    let fs_type = fields[separator + 1];
    let super_options = fields[separator + 3];
    if fs_type == "cgroup" && controllerMatch(super_options, controller) {
        (1, true)
    } else if fs_type == "cgroup2" {
        (2, true)
    } else {
        (0, false)
    }
}

/// 扫描 mountinfo，收集匹配控制器的 v1/v2 挂载路径与版本列表。
pub(crate) fn getCgroupDetails(
    path: &Path,
    cgroup_root: &str,
    controller: &str,
) -> Result<(Vec<String>, Vec<i32>)> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to read mounts info from file: {}", path.display()))?;
    let mut v1 = None;
    let mut v2 = None;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<_> = line.split_whitespace().collect();
        let (version, found) = detectCgroupVersion(&fields, controller);
        if !found {
            continue;
        }
        let mount = fields[4];
        if version == 2 {
            v2 = Some(mount.to_owned());
            continue;
        }
        let namespace_root = Path::new(fields[3]);
        if fields[3].contains("..") {
            continue;
        }
        if let Ok(relative) = Path::new(cgroup_root).strip_prefix(namespace_root) {
            v1 = Some(
                Path::new(mount)
                    .join(relative)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    match (v1, v2) {
        (Some(a), Some(b)) => Ok((vec![a, b], vec![1, 2])),
        (Some(a), None) => Ok((vec![a], vec![1])),
        (None, Some(b)) => Ok((vec![b], vec![2])),
        _ => Err(anyhow!("failed to detect cgroup root mount and version")),
    }
}

/// 读取文件全文并解析为 `T`，附带读/解析失败上下文。
fn parse_file<T>(path: &Path, read_desc: &str, parse_desc: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let value =
        fs::read_to_string(path).with_context(|| format!("{read_desc} at {}", path.display()))?;
    value
        .trim()
        .parse()
        .with_context(|| format!("{parse_desc} at {}", path.display()))
}

/// v1：读 `(period, quota)`，来自 cfs_period_us / cfs_quota_us。
pub(crate) fn detectCPUQuotaInV1(root: &Path) -> Result<(i64, i64)> {
    let quota = parse_file(
        &root.join(cgroupV1CPUQuota),
        "error when reading cpu quota from cgroup v1",
        "error when parsing cpu quota from cgroup v1",
    )?;
    let period = parse_file(
        &root.join(cgroupV1CPUPeriod),
        "error when reading cpu period from cgroup v1",
        "error when parsing cpu period from cgroup v1",
    )?;
    Ok((period, quota))
}

/// v1：读 `(stime, utime)`，来自 cpuacct.usage_sys / usage_user。
pub(crate) fn detectCPUUsageInV1(root: &Path) -> Result<(u64, u64)> {
    let stime = parse_file(
        &root.join(cgroupV1CPUSysUsage),
        "error when reading cpu system time from cgroup v1",
        "error when parsing cpu system time from cgroup v1",
    )?;
    let utime = parse_file(
        &root.join(cgroupV1CPUUserUsage),
        "error when reading cpu user time from cgroup v1",
        "error when parsing cpu user time from cgroup v1",
    )?;
    Ok((stime, utime))
}

/// v2：解析 `cpu.max`；`max` 记为 quota=-1。
pub(crate) fn detectCPUQuotaInV2(root: &Path) -> Result<(i64, i64)> {
    let path = root.join(cgroupV2CPUMax);
    let value = fs::read_to_string(&path).with_context(|| {
        format!(
            "error when read cpu quota from cgroup v2 at {}",
            path.display()
        )
    })?;
    let fields: Vec<_> = value.split_whitespace().collect();
    if fields.is_empty() || fields.len() > 2 {
        return Err(anyhow!(
            "unexpected format when reading cpu quota from cgroup v2 at {}: {}",
            path.display(),
            value
        ));
    }
    // "max" 表示无硬限制，与 v1 的 -1 对齐。
    let quota = if fields[0] == "max" {
        -1
    } else {
        fields[0].parse().with_context(|| {
            format!(
                "error when reading cpu quota from cgroup v2 at {}",
                path.display()
            )
        })?
    };
    let period = if fields.len() == 2 {
        fields[1].parse().with_context(|| {
            format!(
                "error when reading cpu period from cgroup v2 at {}",
                path.display()
            )
        })?
    } else {
        0
    };
    Ok((period, quota))
}

/// v2：从 `cpu.stat` 提取 system_usec / user_usec。
pub(crate) fn detectCPUUsageInV2(root: &Path) -> Result<(u64, u64)> {
    let path = root.join(cgroupV2CPUStat);
    let file = fs::File::open(&path)
        .with_context(|| format!("can't read cpu usage from cgroup v2 at {}", path.display()))?;
    let (mut stime, mut utime) = (0, 0);
    for line in BufReader::new(file).lines() {
        let line = line?;
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 2 || !matches!(fields[0], "user_usec" | "system_usec") {
            continue;
        }
        let value = fields[1].parse::<u64>().with_context(|| {
            format!(
                "can't read cpu usage {} from cgroup v1 at {}",
                fields[0],
                path.display()
            )
        })?;
        if fields[0] == "user_usec" {
            utime = value;
        } else {
            stime = value;
        }
    }
    Ok((stime, utime))
}

/// 读取单值文件；字面量 `max` 映射为 `i64::MAX as u64`。
pub(crate) fn readInt64Value(root: &Path, filename: &str, version: i32) -> Result<u64> {
    let path = root.join(filename);
    let value = fs::read_to_string(&path)
        .with_context(|| format!("can't read {filename} from cgroup v{version}"))?;
    let value = value
        .lines()
        .next()
        // Go's scanner leaves the named uint64 result at zero for an empty
        // file and errors.Wrapf(nil, ...) preserves the nil error.
        .unwrap_or("0")
        .trim();
    if value == "max" {
        return Ok(i64::MAX as u64);
    }
    value
        .parse()
        .with_context(|| format!("failed to parse value in {filename} from cgroup v{version}"))
}

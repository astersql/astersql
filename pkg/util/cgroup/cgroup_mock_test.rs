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

// cgroup 表驱动 mock 测试：在临时目录构造假 proc/cgroup 文件树。
//
// 覆盖内存用量/限额/inactive file、CPU 配额与 period/quota 在 v1/v2、
// 命名空间挂载、混合挂载及控制器字段顺序等场景，对齐 Go 断言。

use super::{
    CPUUsage, Unknown, V1, V2, Version, getCgroupCPU, getCgroupCPUPeriodAndQuota,
    getCgroupMemInactiveFileUsage, getCgroupMemLimit, getCgroupMemUsage,
};
use regex::Regex;
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;

// is_error 对应 Go 的 isError。
// nil/空字符串组合表示无错误；非空错误通过正则匹配 Error() 文本。
/// 对应 Go `isError`：无错且正则为空，或错误文本匹配正则。
fn is_error(err: Option<&str>, re: &str) -> bool {
    if err.is_none() && re.is_empty() {
        return true;
    }
    if err.is_none() || re.is_empty() {
        return false;
    }

    Regex::new(re).is_ok_and(|pattern| pattern.is_match(err.unwrap()))
}

#[derive(Default)]
/// 内存用量/inactive file 表驱动用例。
struct MemUsageCase {
    name: &'static str,
    paths: BTreeMap<&'static str, &'static str>,
    err_msg: &'static str,
    value: u64,
    warn: &'static str,
}

/// 内存限额表驱动用例（含期望版本）。
struct MemLimitCase {
    name: &'static str,
    paths: BTreeMap<&'static str, &'static str>,
    err_msg: &'static str,
    limit: u64,
    warn: &'static str,
    version: Version,
}

impl Default for MemLimitCase {
    fn default() -> Self {
        Self {
            name: "",
            paths: BTreeMap::new(),
            err_msg: "",
            limit: 0,
            warn: "",
            version: Unknown,
        }
    }
}

#[derive(Default)]
/// CPU 表驱动用例：period/quota 与 user/system 用量。
struct CpuCase {
    name: &'static str,
    paths: BTreeMap<&'static str, &'static str>,
    err_msg: &'static str,
    period: i64,
    quota: i64,
    user: u64,
    system: u64,
}

/// 将路径-内容切片转为有序映射，便于构造假文件系统。
fn map(entries: &[(&'static str, &'static str)]) -> BTreeMap<&'static str, &'static str> {
    entries.iter().copied().collect()
}

// test_cgroups_get_memory_usage 对应 Go 的 TestCgroupsGetMemoryUsage。
// 表驱动覆盖缺 cgroup 文件、无 memory controller、v1/v2 成功和 v2 解析失败等场景。
#[test]
fn test_cgroups_get_memory_usage() {
    let cases = vec![
        MemUsageCase {
            err_msg: "failed to read memory cgroup from cgroups file:",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithoutMemoryController),
                ("/proc/self/mountinfo", v1MountsWithoutMemController),
            ]),
            warn: "no cgroup memory controller detected",
            value: 0,
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[("/proc/self/cgroup", v1CgroupWithMemoryController)]),
            err_msg: "failed to read mounts info from file:",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryController),
                ("/proc/self/mountinfo", v1MountsWithoutMemController),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryController),
                ("/proc/self/mountinfo", v1MountsWithMemController),
                (
                    "/sys/fs/cgroup/memory/memory.usage_in_bytes",
                    v1MemoryUsageInBytes,
                ),
            ]),
            value: 276328448,
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryControllerNS),
                ("/proc/self/mountinfo", v1MountsWithMemControllerNS),
                (
                    "/sys/fs/cgroup/memory/cgroup_test/memory.usage_in_bytes",
                    v1MemoryUsageInBytes,
                ),
            ]),
            value: 276328448,
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
            ]),
            err_msg: "can't read memory.current from cgroup v2",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.current",
                    "unparsable\n",
                ),
            ]),
            err_msg: "failed to parse value in memory.current from cgroup v2",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.current",
                    "276328448",
                ),
            ]),
            value: 276328448,
            ..Default::default()
        },
    ];

    for tc in cases {
        let dir = create_files(&tc.paths);
        let (limit, err) = value_or_error(getCgroupMemUsage(dir.path()));
        assert!(
            is_error(err.as_deref(), tc.err_msg),
            "{:?} {}",
            err,
            tc.err_msg
        );
        assert_eq!(tc.value, limit);
    }
}

// test_cgroups_get_memory_inactive_file_usage 对应 Go 的 TestCgroupsGetMemoryInactiveFileUsage。
// 它复用 memory controller 探测路径，但读取 memory.stat 中的 inactive_file 字段。
#[test]
fn test_cgroups_get_memory_inactive_file_usage() {
    let cases = vec![
        MemUsageCase {
            err_msg: "failed to read memory cgroup from cgroups file:",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithoutMemoryController),
                ("/proc/self/mountinfo", v1MountsWithoutMemController),
            ]),
            warn: "no cgroup memory controller detected",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[("/proc/self/cgroup", v1CgroupWithMemoryController)]),
            err_msg: "failed to read mounts info from file:",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryController),
                ("/proc/self/mountinfo", v1MountsWithoutMemController),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryController),
                ("/proc/self/mountinfo", v1MountsWithMemController),
                ("/sys/fs/cgroup/memory/memory.stat", v1MemoryStat),
            ]),
            value: 1363746816,
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryControllerNS),
                ("/proc/self/mountinfo", v1MountsWithMemControllerNS),
                (
                    "/sys/fs/cgroup/memory/cgroup_test/memory.stat",
                    v1MemoryStat,
                ),
            ]),
            value: 1363746816,
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithEccentricMemoryController),
                ("/proc/self/mountinfo", v1MountsWithEccentricMemController),
                ("/sys/fs/cgroup/memory/memory.stat", v1MemoryStat),
            ]),
            value: 1363746816,
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
            ]),
            err_msg: "can't read file memory.stat from cgroup v2",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.stat",
                    "inactive_file unparsable\n",
                ),
            ]),
            err_msg: "can't read \"inactive_file\" memory stat from cgroup v2 in memory.stat",
            ..Default::default()
        },
        MemUsageCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.stat",
                    v2MemoryStat,
                ),
            ]),
            value: 1363746816,
            ..Default::default()
        },
    ];

    for tc in cases {
        let dir = create_files(&tc.paths);
        let (limit, err) = value_or_error(getCgroupMemInactiveFileUsage(dir.path()));
        assert!(
            is_error(err.as_deref(), tc.err_msg),
            "{:?} {}",
            err,
            tc.err_msg
        );
        assert_eq!(tc.value, limit);
    }
}

// test_cgroups_get_memory_limit 对应 Go 的 TestCgroupsGetMemoryLimit。
// 该测试还断言成功时的 cgroup 版本，覆盖 v1 hierarchical_memory_limit、v2 memory.max 和 "max" 哨兵值。
#[test]
fn test_cgroups_get_memory_limit() {
    let cases = vec![
        MemLimitCase {
            err_msg: "failed to read memory cgroup from cgroups file:",
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithoutMemoryController),
                ("/proc/self/mountinfo", v1MountsWithoutMemController),
            ]),
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[("/proc/self/cgroup", v1CgroupWithMemoryController)]),
            err_msg: "failed to read mounts info from file:",
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryController),
                ("/proc/self/mountinfo", v1MountsWithoutMemController),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryController),
                ("/proc/self/mountinfo", v1MountsWithMemController),
                ("/sys/fs/cgroup/memory/memory.stat", v1MemoryStat),
            ]),
            limit: 2936016896,
            version: V1,
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithMemoryControllerNS),
                ("/proc/self/mountinfo", v1MountsWithMemControllerNS),
                (
                    "/sys/fs/cgroup/memory/cgroup_test/memory.stat",
                    v1MemoryStat,
                ),
            ]),
            limit: 2936016896,
            version: V1,
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
            ]),
            err_msg: "can't read memory.max from cgroup v2",
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.max",
                    "unparsable\n",
                ),
            ]),
            err_msg: "failed to parse value in memory.max from cgroup v2",
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.max",
                    "1073741824\n",
                ),
            ]),
            limit: 1073741824,
            version: V2,
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/memory.max",
                    "max\n",
                ),
            ]),
            limit: 9223372036854775807,
            version: V2,
            ..Default::default()
        },
        MemLimitCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithEccentricMemoryController),
                ("/proc/self/mountinfo", v1MountsWithEccentricMemController),
                ("/sys/fs/cgroup/memory/memory.stat", v1MemoryStat),
            ]),
            limit: 2936016896,
            version: V1,
            ..Default::default()
        },
    ];

    for tc in cases {
        let dir = create_files(&tc.paths);
        let (limit, version, err) = match getCgroupMemLimit(dir.path()) {
            Ok((limit, version)) => (limit, version, None),
            Err(err) => (0, Unknown, Some(err.to_string())),
        };
        assert!(
            is_error(err.as_deref(), tc.err_msg),
            "{:?} {}",
            err,
            tc.err_msg
        );
        assert_eq!(tc.limit, limit);
        if err.is_none() {
            assert_eq!(tc.version, version);
        }
    }
}

// test_cgroups_get_cpu 对应 Go 的 TestCgroupsGetCPU。
// Go 跑两轮：第二轮把 "cpu,cpuacct" 改成 "cpuacct,cpu"，确认 controller 顺序不影响解析。
#[test]
/// 正向与反转控制器顺序各跑一遍 CPU 表驱动。
fn test_cgroups_get_cpu() {
    for reverse_controller_order in [false, true] {
        test_cgroups_get_cpu_inner(reverse_controller_order);
        test_cgroups_get_cpu_period_and_quota(reverse_controller_order);
    }
}

/// CPU 表驱动核心：可反转控制器字段顺序以覆盖匹配边界。
fn test_cgroups_get_cpu_inner(reverse_controller_order: bool) {
    let cases = vec![
        CpuCase {
            err_msg: "failed to read cpu,cpuacct cgroup from cgroups file:",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithoutCPUController),
                ("/proc/self/mountinfo", v1MountsWithoutCPUController),
            ]),
            err_msg: "no cpu controller detected",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[("/proc/self/cgroup", v1CgroupWithCPUController)]),
            err_msg: "failed to read mounts info from file:",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUController),
                ("/proc/self/mountinfo", v1MountsWithoutCPUController),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v1_paths("", "12345", "67890", "123", "456")),
            quota: 12345,
            period: 67890,
            system: 123,
            user: 456,
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v1_paths("crdb_test/", "12345", "67890", "123", "456")),
            quota: 12345,
            period: 67890,
            system: 123,
            user: 456,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                (
                    "/proc/self/cgroup",
                    v1CgroupWithCPUControllerNSMountRelRemount,
                ),
                (
                    "/proc/self/mountinfo",
                    v1MountsWithCPUControllerNSMountRelRemount,
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_quota_us",
                    "12345",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_period_us",
                    "67890",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpuacct.usage_sys",
                    "123",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpuacct.usage_user",
                    "456",
                ),
            ]),
            quota: 12345,
            period: 67890,
            system: 123,
            user: 456,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUControllerNS2),
                ("/proc/self/mountinfo", v1MountsWithCPUControllerNS2),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_quota_us",
                    "12345",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_period_us",
                    "67890",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpuacct.usage_sys",
                    "123",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpuacct.usage_user",
                    "456",
                ),
            ]),
            quota: 12345,
            period: 67890,
            system: 123,
            user: 456,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUController),
                ("/proc/self/mountinfo", v1MountsWithCPUController),
                ("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us", "-1"),
                ("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_period_us", "67890"),
            ]),
            err_msg: "error when reading cpu system time from cgroup v1",
            quota: -1,
            period: 67890,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUControllerNSMountRel),
                ("/proc/self/mountinfo", v1MountsWithCPUControllerNSMountRel),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
            ]),
            err_msg: "error when read cpu quota from cgroup v2",
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v2_paths("foo bar\n", "")),
            err_msg: "error when reading cpu quota from cgroup v2 at",
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v2_paths(
                "100 1000\n",
                "user_usec 100\nsystem_usec 200",
            )),
            quota: 100,
            period: 1000,
            user: 100,
            system: 200,
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v2_paths(
                "max 1000\n",
                "user_usec 100\nsystem_usec 200",
            )),
            quota: -1,
            period: 1000,
            user: 100,
            system: 200,
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v2_period_paths("100 1000\n")),
            err_msg: "can't read cpu usage from cgroup v2",
            quota: 100,
            period: 1000,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", MixCgroup),
                ("/proc/self/mountinfo", MixMounts),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/user.slice/cpu.cfs_quota_us",
                    "12345",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/user.slice/cpu.cfs_period_us",
                    "67890",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/user.slice/cpuacct.usage_sys",
                    "123",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/user.slice/cpuacct.usage_user",
                    "456",
                ),
            ]),
            quota: 12345,
            period: 67890,
            system: 123,
            user: 456,
            ..Default::default()
        },
    ];

    for tc in cases {
        let dir = create_cpu_files(&tc.paths, reverse_controller_order);
        let (cpuusage, err) = match getCgroupCPU(dir.path()) {
            Ok(cpu) => (cpu, None),
            Err(err) => {
                let (period, quota) = getCgroupCPUPeriodAndQuota(dir.path()).unwrap_or_default();
                (
                    CPUUsage {
                        Period: period,
                        Quota: quota,
                        ..Default::default()
                    },
                    Some(err.to_string()),
                )
            }
        };
        assert!(
            is_error(err.as_deref(), tc.err_msg),
            "{:?} {}",
            err,
            tc.err_msg
        );
        assert_eq!(tc.quota, cpuusage.Quota);
        assert_eq!(tc.period, cpuusage.Period);
        assert_eq!(tc.system, cpuusage.Stime);
        assert_eq!(tc.user, cpuusage.Utime);
    }
}

/// 仅校验 period/quota 探测路径（不含 usage）。
fn test_cgroups_get_cpu_period_and_quota(reverse_controller_order: bool) {
    let cases = vec![
        CpuCase {
            err_msg: "failed to read cpu cgroup from cgroups file:",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithoutCPUController),
                ("/proc/self/mountinfo", v1MountsWithoutCPUController),
            ]),
            err_msg: "no cpu controller detected",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUControllerNS),
                ("/proc/self/mountinfo", v1MountsWithCPUControllerNS),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_quota_us",
                    "12345",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_period_us",
                    "67890",
                ),
            ]),
            quota: 12345,
            period: 67890,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUControllerNSMountRel),
                ("/proc/self/mountinfo", v1MountsWithCPUControllerNSMountRel),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                (
                    "/proc/self/cgroup",
                    v1CgroupWithCPUControllerNSMountRelRemount,
                ),
                (
                    "/proc/self/mountinfo",
                    v1MountsWithCPUControllerNSMountRelRemount,
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_quota_us",
                    "12345",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_period_us",
                    "67890",
                ),
            ]),
            quota: 12345,
            period: 67890,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUControllerNS2),
                ("/proc/self/mountinfo", v1MountsWithCPUControllerNS2),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_quota_us",
                    "12345",
                ),
                (
                    "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_period_us",
                    "67890",
                ),
            ]),
            quota: 12345,
            period: 67890,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUController),
                ("/proc/self/mountinfo", v1MountsWithCPUController),
                ("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us", "-1"),
                ("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_period_us", "67890"),
            ]),
            quota: -1,
            period: 67890,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[("/proc/self/cgroup", v1CgroupWithCPUController)]),
            err_msg: "failed to read mounts info from file:",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUController),
                ("/proc/self/mountinfo", v1MountsWithoutCPUController),
            ]),
            err_msg: "failed to detect cgroup root mount and version",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v1CgroupWithCPUController),
                ("/proc/self/mountinfo", v1MountsWithCPUController),
                ("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us", "12345"),
                ("/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_period_us", "67890"),
            ]),
            quota: 12345,
            period: 67890,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
            ]),
            err_msg: "error when read cpu quota from cgroup v2",
            ..Default::default()
        },
        CpuCase {
            paths: map(&cpu_v2_period_paths("foo bar\n")),
            err_msg: "error when reading cpu quota from cgroup v2 at",
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/cpu.max",
                    "100 1000\n",
                ),
            ]),
            quota: 100,
            period: 1000,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/cpu.max",
                    "max 1000\n",
                ),
            ]),
            quota: -1,
            period: 1000,
            ..Default::default()
        },
        CpuCase {
            paths: map(&[
                ("/proc/self/cgroup", v2CgroupWithMemoryController),
                ("/proc/self/mountinfo", v2Mounts),
                (
                    "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/cpu.max",
                    "100 1000\n",
                ),
            ]),
            quota: 100,
            period: 1000,
            ..Default::default()
        },
    ];

    for tc in cases {
        let dir = create_cpu_files(&tc.paths, reverse_controller_order);
        let (period, quota, err) = match getCgroupCPUPeriodAndQuota(dir.path()) {
            Ok((period, quota)) => (period, quota, None),
            Err(err) => (0, 0, Some(err.to_string())),
        };
        assert!(
            is_error(err.as_deref(), tc.err_msg),
            "{:?} {}",
            err,
            tc.err_msg
        );
        assert_eq!(tc.quota, quota);
        assert_eq!(tc.period, period);
    }
}

// create_files 对应 Go 的 createFiles。
// Go 用 t.TempDir 创建隔离根目录，再把绝对路径 join 到临时根下，模拟 /proc 与 /sys 文件树。
/// 将 Result 拆成 (值或 0, 可选错误字符串)。
fn value_or_error(result: anyhow::Result<u64>) -> (u64, Option<String>) {
    match result {
        Ok(value) => (value, None),
        Err(err) => (0, Some(err.to_string())),
    }
}

/// 在临时目录按绝对风格路径写入假文件。
fn create_files(paths: &BTreeMap<&str, &str>) -> TempDir {
    let dir = tempfile::tempdir().expect("create temporary cgroup root");
    for (path, data) in paths {
        let path = path.trim_start_matches('/');
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().expect("fixture path has parent"))
            .expect("create cgroup fixture directory");
        fs::write(&path, data).expect("write cgroup fixture");
    }
    dir
}

/// 写入 CPU 相关假文件；可选反转 cgroup 行中控制器列表顺序。
fn create_cpu_files(paths: &BTreeMap<&str, &str>, reverse_controller_order: bool) -> TempDir {
    if !reverse_controller_order {
        return create_files(paths);
    }
    let dir = tempfile::tempdir().expect("create temporary cgroup root");
    for (path, data) in paths {
        let data = if *path == "/proc/self/cgroup" {
            data.replace("cpu,cpuacct", "cpuacct,cpu")
        } else if *path == "/proc/self/mountinfo" {
            data.replace("rw,cpu,cpuacct", "rw,cpuacct,cpu")
        } else {
            (*data).to_owned()
        };
        let path = dir.path().join(path.trim_start_matches('/'));
        fs::create_dir_all(path.parent().expect("fixture path has parent"))
            .expect("create cgroup fixture directory");
        fs::write(path, data).expect("write cgroup fixture");
    }
    dir
}

/// 组装一组 v1 CPU 相关假文件路径与内容。
fn cpu_v1_paths(
    prefix: &'static str,
    quota: &'static str,
    period: &'static str,
    system: &'static str,
    user: &'static str,
) -> [(&'static str, &'static str); 6] {
    let (cgroup, mounts) = if prefix.is_empty() {
        (v1CgroupWithCPUController, v1MountsWithCPUController)
    } else {
        (v1CgroupWithCPUControllerNS, v1MountsWithCPUControllerNS)
    };
    [
        ("/proc/self/cgroup", cgroup),
        ("/proc/self/mountinfo", mounts),
        (
            if prefix.is_empty() {
                "/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us"
            } else {
                "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_quota_us"
            },
            quota,
        ),
        (
            if prefix.is_empty() {
                "/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_period_us"
            } else {
                "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpu.cfs_period_us"
            },
            period,
        ),
        (
            if prefix.is_empty() {
                "/sys/fs/cgroup/cpu,cpuacct/cpuacct.usage_sys"
            } else {
                "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpuacct.usage_sys"
            },
            system,
        ),
        (
            if prefix.is_empty() {
                "/sys/fs/cgroup/cpu,cpuacct/cpuacct.usage_user"
            } else {
                "/sys/fs/cgroup/cpu,cpuacct/crdb_test/cpuacct.usage_user"
            },
            user,
        ),
    ]
}

/// 组装仅含 period/quota 探测所需的 v2 路径。
fn cpu_v2_period_paths(cpu_max: &'static str) -> [(&'static str, &'static str); 3] {
    [
        ("/proc/self/cgroup", v2CgroupWithMemoryController),
        ("/proc/self/mountinfo", v2Mounts),
        (
            "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/cpu.max",
            cpu_max,
        ),
    ]
}

/// 组装含 cpu.max 与 cpu.stat 的完整 v2 CPU 路径集。
fn cpu_v2_paths(
    cpu_max: &'static str,
    cpu_stat: &'static str,
) -> [(&'static str, &'static str); 4] {
    [
        ("/proc/self/cgroup", v2CgroupWithMemoryController),
        ("/proc/self/mountinfo", v2Mounts),
        (
            "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/cpu.max",
            cpu_max,
        ),
        (
            "/sys/fs/cgroup/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope/cpu.stat",
            cpu_stat,
        ),
    ]
}

// 以下 fixture 对应 Go var/const 块。长 mountinfo 保留关键 controller 行，不追求可运行解析。
/// 非常规 memory 控制器字段顺序的假 `/proc/self/cgroup` 片段。
const v1CgroupWithEccentricMemoryController: &str = r#"13:devices:/system.slice/containerd.service/kubepods-burstable.slice:cri-containerd:container-id
11:cpu,cpuacct:/system.slice/containerd.service/kubepods-burstable.slice:cri-containerd:container-id
5:memory:/system.slice/containerd.service/kubepods-burstable.slice:cri-containerd:container-id
0::/
"#;

const v1MountsWithEccentricMemController: &str = r#"2305 2304 0:139 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime - tmpfs tmpfs rw,mode=755
2310 2305 0:31 /system.slice/containerd.service/kubepods-burstable.slice:cri-containerd:container-id /sys/fs/cgroup/memory ro,nosuid,nodev,noexec,relatime master:9 - cgroup cgroup rw,memory
2316 2305 0:37 /system.slice/containerd.service/kubepods-burstable.slice:cri-containerd:container-id /sys/fs/cgroup/cpu,cpuacct ro,nosuid,nodev,noexec,relatime master:15 - cgroup cgroup rw,cpu,cpuacct
"#;

/// 含 memory 控制器的典型 v1 cgroup 文本。
static v1CgroupWithMemoryController: &str = r#"11:blkio:/kubepods/besteffort/pod/cid
8:cpu,cpuacct:/kubepods/besteffort/pod/cid
5:memory:/kubepods/besteffort/pod/cid
1:name=systemd:/kubepods/besteffort/pod/cid
"#;

/// 不含 memory 控制器的 v1 cgroup 文本。
static v1CgroupWithoutMemoryController: &str = r#"10:blkio:/kubepods/besteffort/pod/cid
7:cpu,cpuacct:/kubepods/besteffort/pod/cid
1:name=systemd:/kubepods/besteffort/pod/cid
"#;

/// 含 cpu/cpuacct 控制器的 v1 cgroup 文本。
static v1CgroupWithCPUController: &str = r#"11:blkio:/kubepods/besteffort/pod/cid
8:cpu,cpuacct:/kubepods/besteffort/pod/cid
5:memory:/kubepods/besteffort/pod/cid
1:name=systemd:/kubepods/besteffort/pod/cid
"#;

/// 不含 cpu 控制器的 v1 cgroup 文本。
static v1CgroupWithoutCPUController: &str = r#"10:blkio:/kubepods/besteffort/pod/cid
7:pids:/kubepods/besteffort/pod/cid
5:memory:/kubepods/besteffort/pod/cid
1:name=systemd:/kubepods/besteffort/pod/cid
"#;

/// 统一层次（v2）下的假 cgroup 路径行。
static v2CgroupWithMemoryController: &str = "0::/machine.slice/libpod-f1c6b44c0d61f273952b8daecf154cee1be2d503b7e9184ebf7fcaf48e139810.scope\n";

/// 含 memory 挂载的假 mountinfo。
static v1MountsWithMemController: &str = r#"703 702 0:99 / /sys/fs/cgroup ro,nosuid,nodev,noexec,relatime - tmpfs tmpfs rw,mode=755
733 703 0:28 /kubepods/besteffort/pod/cid /sys/fs/cgroup/memory ro,nosuid,nodev,noexec,relatime master:13 - cgroup cgroup rw,memory
736 703 0:31 /kubepods/besteffort/pod/cid /sys/fs/cgroup/cpu,cpuacct ro,nosuid,nodev,noexec,relatime master:16 - cgroup cgroup rw,cpu,cpuacct
"#;

/// 不含 memory 挂载的假 mountinfo。
static v1MountsWithoutMemController: &str = r#"703 702 0:99 / /sys/fs/cgroup ro,nosuid,nodev,noexec,relatime - tmpfs tmpfs rw,mode=755
736 703 0:31 /kubepods/besteffort/pod/cid /sys/fs/cgroup/cpu,cpuacct ro,nosuid,nodev,noexec,relatime master:16 - cgroup cgroup rw,cpu,cpuacct
"#;

/// 含 cpu,cpuacct 挂载的假 mountinfo。
static v1MountsWithCPUController: &str = r#"703 702 0:99 / /sys/fs/cgroup ro,nosuid,nodev,noexec,relatime - tmpfs tmpfs rw,mode=755
736 703 0:31 /kubepods/besteffort/pod/cid /sys/fs/cgroup/cpu,cpuacct ro,nosuid,nodev,noexec,relatime master:16 - cgroup cgroup rw,cpu,cpuacct
"#;

/// 不含 cpu 挂载的假 mountinfo。
static v1MountsWithoutCPUController: &str = r#"703 702 0:99 / /sys/fs/cgroup ro,nosuid,nodev,noexec,relatime - tmpfs tmpfs rw,mode=755
733 703 0:28 /kubepods/besteffort/pod/cid /sys/fs/cgroup/memory ro,nosuid,nodev,noexec,relatime master:13 - cgroup cgroup rw,memory
"#;

/// 含 cgroup2 挂载的假 mountinfo。
static v2Mounts: &str = r#"371 344 0:35 / / rw,relatime - overlay overlay rw,lowerdir=/overlay/lower,upperdir=/overlay/upper,workdir=/overlay/work
383 374 0:25 / /sys/fs/cgroup ro,nosuid,nodev,noexec,relatime - cgroup2 cgroup2 rw,seclabel
"#;

/// 样例 v1 memory.stat 内容。
static v1MemoryStat: &str = r#"cache 784113664
rss 1703952384
inactive_file 1363746816
hierarchical_memory_limit 2936016896
hierarchical_memsw_limit 9223372036854771712
total_inactive_file 1363746816
"#;

/// 样例 v2 memory.stat 内容。
static v2MemoryStat: &str = r#"anon 784113664
file 1703952384
inactive_anon 1363746816
inactive_file 1363746816
active_file 2936016896
unevictable 9223372036854771712
"#;

/// 样例 v1 `memory.usage_in_bytes`。
static v1MemoryUsageInBytes: &str = "276328448";

// Go 注释说明：mountinfo 和 cgroup 都相对 cgroup namespace root 展示，这里覆盖两者不完全一致的 k8s pod 场景。
/// 命名空间内相对路径的 memory cgroup 行。
static v1CgroupWithMemoryControllerNS: &str = "12:memory:/cgroup_test";
/// 与 NS memory cgroup 对应的挂载行。
static v1MountsWithMemControllerNS: &str = "50 35 0:44 / /sys/fs/cgroup/memory rw,nosuid,nodev,noexec,relatime shared:25 - cgroup cgroup rw,memory";

// Go 注释说明：CPU controller 的 /proc/self/mountinfo 与 /proc/self/cgroup 路径不同。
/// 命名空间内 CPU 控制器路径。
static v1CgroupWithCPUControllerNS: &str = "5:cpu,cpuacct:/crdb_test";
/// 与 NS CPU 路径对应的挂载行。
static v1MountsWithCPUControllerNS: &str = "43 35 0:37 / /sys/fs/cgroup/cpu,cpuacct rw,nosuid,nodev,noexec,relatime shared:18 - cgroup cgroup rw,cpu,cpuacct";

// Same as above but with unshare -C; Go 期望无法判断 mount 位置。
/// 含相对路径 `..` 挂载场景的 cgroup 行。
static v1CgroupWithCPUControllerNSMountRel: &str = "5:cpu,cpuacct:/";
/// namespace root 含 `..`（应被跳过）的挂载行。
static v1MountsWithCPUControllerNSMountRel: &str = "43 35 0:37 /.. /sys/fs/cgroup/cpu,cpuacct rw,nosuid,nodev,noexec,relatime shared:18 - cgroup cgroup rw,cpu,cpuacct";

// Same as above but with mounting the cgroup fs one more time in the NS。
/// remount 场景下的 CPU cgroup 行。
static v1CgroupWithCPUControllerNSMountRelRemount: &str = "5:cpu,cpuacct:/";
/// 同时含非法 `..` 与合法 remount 的 mountinfo。
static v1MountsWithCPUControllerNSMountRelRemount: &str = r#"43 35 0:37 /.. /sys/fs/cgroup/cpu,cpuacct rw,nosuid,nodev,noexec,relatime shared:18 - cgroup cgroup rw,cpu,cpuacct
161 43 0:37 / /sys/fs/cgroup/cpu,cpuacct/crdb_test rw,relatime shared:95 - cgroup none rw,cpu,cpuacct
"#;

// Same as above but exiting the NS w/o unmounting。
/// 另一组 NS 路径（挂载点更深）的 cgroup 行。
static v1CgroupWithCPUControllerNS2: &str = "5:cpu,cpuacct:/crdb_test";
/// 挂载点带子路径的 CPU mountinfo。
static v1MountsWithCPUControllerNS2: &str = "161 43 0:37 /crdb_test /sys/fs/cgroup/cpu,cpuacct/crdb_test rw,relatime shared:95 - cgroup none rw,cpu,cpuacct";

/// 混合 v1 控制器 + v2 unified 的假 cgroup 文本。
static MixCgroup: &str = r#"12:hugetlb:/
11:memory:/user.slice/user-1006.slice/session-17838.scope
7:cpu,cpuacct:/user.slice
0::/user.slice/user-1006.slice/session-17838.scope
"#;

/// 混合 v1 子挂载与 cgroup2 的假 mountinfo。
static MixMounts: &str = r#"34 25 0:28 / /sys/fs/cgroup ro,nosuid,nodev,noexec shared:9 - tmpfs tmpfs ro,mode=755
44 34 0:38 / /sys/fs/cgroup/cpu,cpuacct rw,nosuid,nodev,noexec,relatime shared:20 - cgroup cgroup rw,cpu,cpuacct
48 34 0:42 / /sys/fs/cgroup/memory rw,nosuid,nodev,noexec,relatime shared:24 - cgroup cgroup rw,memory
"#;

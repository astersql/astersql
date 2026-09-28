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

// cgroup CPU/内存探测的迁移期单元测试（可注入临时 root）。
//
// 通过拼装假 `/proc` 与 cgroup 文件，对齐 Go 侧控制器匹配、v1/v2 解析与报错上下文。

#[path = "cgroup.rs"]
mod cgroup;
pub use cgroup::*;
#[path = "cgroup_cpu.rs"]
mod cgroup_cpu;
use cgroup_cpu::*;
#[path = "cgroup_cpu_linux.rs"]
mod cgroup_cpu_linux;
pub use cgroup_cpu_linux::*;
#[path = "cgroup_memory.rs"]
mod cgroup_memory;
pub use cgroup_memory::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 临时目录序号，避免并发测试路径冲突。
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    /// 测试用临时文件系统根；Drop 时清理。
    struct TempRoot(PathBuf);

    impl TempRoot {
        /// 创建唯一临时目录。
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("tidb-cgroup-migration-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// 相对本 root 写入绝对风格路径（去掉前导 `/`）。
        fn write(&self, absolute: &str, contents: &str) {
            let path = self.0.join(absolute.trim_start_matches('/'));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        /// 返回临时根路径。
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    /// 校验 controllerMatch 与 detectControlPath 与 Go 行为一致。
    fn controller_and_control_path_match_go() {
        assert!(controllerMatch("cpuacct,cpu", "cpu,cpuacct"));
        assert!(controllerMatch("rw,cpuacct,cpu", "cpu,cpuacct"));
        assert!(!controllerMatch("cpu", "cpu,cpuacct"));

        let root = TempRoot::new();
        root.write(
            "/proc/self/cgroup",
            "0::/unified\n2:cpuacct,cpu:/docker/a:b\n",
        );
        let path = detectControlPath(&root.path().join("proc/self/cgroup"), "cpu,cpuacct").unwrap();
        assert_eq!(path, "/docker/a:b");
    }

    #[test]
    /// 直接读写 v1/v2 CPU 文件解析结果。
    fn v1_and_v2_cpu_files_match_go() {
        let root = TempRoot::new();
        root.write("/cpu.cfs_quota_us", "150000\n");
        root.write("/cpu.cfs_period_us", "100000\n");
        root.write("/cpuacct.usage_sys", "21\n");
        root.write("/cpuacct.usage_user", "34\n");
        assert_eq!(detectCPUQuotaInV1(root.path()).unwrap(), (100000, 150000));
        assert_eq!(detectCPUUsageInV1(root.path()).unwrap(), (21, 34));

        root.write("/cpu.max", "max 100000\n");
        root.write("/cpu.stat", "usage_usec 99\nuser_usec 12\nsystem_usec 7\n");
        assert_eq!(detectCPUQuotaInV2(root.path()).unwrap(), (100000, -1));
        assert_eq!(detectCPUUsageInV2(root.path()).unwrap(), (7, 12));
    }

    #[test]
    /// 端到端：假 mountinfo + cpu.max/stat 走 getCgroupCPU。
    fn cgroup_v2_cpu_end_to_end_matches_go() {
        let root = TempRoot::new();
        root.write("/proc/self/cgroup", "0::/machine.slice/demo.scope\n");
        root.write(
            "/proc/self/mountinfo",
            "29 23 0:26 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw\n",
        );
        root.write(
            "/sys/fs/cgroup/machine.slice/demo.scope/cpu.max",
            "250000 100000\n",
        );
        root.write(
            "/sys/fs/cgroup/machine.slice/demo.scope/cpu.stat",
            "user_usec 40\nsystem_usec 20\n",
        );
        let cpu = getCgroupCPU(root.path()).unwrap();
        assert_eq!(
            (cpu.Period, cpu.Quota, cpu.Stime, cpu.Utime),
            (100000, 250000, 20, 40)
        );
        assert_eq!(cpu.CPUShares(), 2.5);
    }

    #[test]
    /// 内存 stat/limit/usage 及 `max` 字面量语义。
    fn memory_values_and_max_match_go() {
        let root = TempRoot::new();
        root.write(
            "/memory.stat",
            "inactive_file 17\nhierarchical_memory_limit 4096\n",
        );
        root.write("/memory.max", "max\n");
        root.write("/memory.current", "2048\n");
        assert_eq!(
            detectMemStatValue(root.path(), "memory.stat", "inactive_file", 2).unwrap(),
            17
        );
        assert_eq!(detectMemLimitInV1(root.path()).unwrap(), 4096);
        assert_eq!(detectMemLimitInV2(root.path()).unwrap(), i64::MAX as u64);
        assert_eq!(detectMemUsageInV2(root.path()).unwrap(), 2048);
    }

    #[test]
    /// 非法格式时错误信息需保留 Go 风格上下文关键词。
    fn malformed_values_keep_go_error_context() {
        let root = TempRoot::new();
        root.write("/cpu.max", "1 2 3\n");
        assert!(
            detectCPUQuotaInV2(root.path())
                .unwrap_err()
                .to_string()
                .contains("unexpected format")
        );
        root.write("/memory.stat", "inactive_file nope\n");
        assert!(
            detectMemStatValue(root.path(), "memory.stat", "inactive_file", 2)
                .unwrap_err()
                .to_string()
                .contains("inactive_file")
        );
    }
}

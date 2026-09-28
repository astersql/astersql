// Copyright 2026 AsterSQL.

// `util/cgroup` crate 入口：读取 Linux cgroup 的 CPU / 内存限额。
//
// 对应 Go `util/cgroup`。cgroup（control group）是 Linux 用于限制与计量
// 进程组资源的机制；本 crate 在 Linux 上走真实实现，非 Linux 走 stub，
// 供执行器等模块感知容器/进程资源上限。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// cgroup 通用探测与路径解析。
mod cgroup;
/// CPU 配额读取的平台无关入口。
mod cgroup_cpu;
#[cfg(target_os = "linux")]
/// Linux 上的 cgroup CPU 配额实现。
mod cgroup_cpu_linux;
#[cfg(not(target_os = "linux"))]
/// 非 Linux 平台的 CPU 配额占位实现。
mod cgroup_cpu_unsupport;
#[cfg(target_os = "linux")]
/// Linux 上的 cgroup 内存限额实现。
mod cgroup_memory;
#[cfg(not(target_os = "linux"))]
/// 非 Linux 平台的内存限额占位实现。
mod cgroup_memory_unsupport;
pub use cgroup::*;
pub(crate) use cgroup_cpu::*;
#[cfg(target_os = "linux")]
pub use cgroup_cpu_linux::*;
#[cfg(not(target_os = "linux"))]
pub use cgroup_cpu_unsupport::*;
#[cfg(target_os = "linux")]
pub use cgroup_memory::*;
#[cfg(not(target_os = "linux"))]
pub use cgroup_memory_unsupport::*;

#[cfg(test)]
#[path = "cgroup_cpu_linux_test.rs"]
/// 对应 Go Linux 容器探测字节语义的回归测试。
mod cgroup_cpu_linux_test;
#[cfg(test)]
#[path = "cgroup_cpu_test.rs"]
/// 对应 Go `cgroup_cpu_test.go` 的 CPU 配额单元测试。
mod cgroup_cpu_test;
#[cfg(test)]
#[path = "cgroup_mock_test.rs"]
/// 使用 mock 文件系统探测 cgroup 路径的测试。
mod cgroup_mock_test;
#[cfg(test)]
#[path = "cgroup_test.rs"]
/// `cgroup.rs` 与 Go 实现的聚焦一致性测试。
mod cgroup_test;
#[cfg(test)]
#[path = "cgroup_cpu_linux_1_aster_unit_test.rs"]
/// AsterSQL 迁移补充的 Linux CPU 单元测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "cgroup_memory_test.rs"]
mod cgroup_memory_test;

#[cfg(test)]
#[path = "cgroup_memory_unsupport_test.rs"]
mod cgroup_memory_unsupport_test;

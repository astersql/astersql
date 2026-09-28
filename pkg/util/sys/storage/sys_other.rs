// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 非 linux/windows/macos 平台的目录容量兜底实现。
//
// 对应 Go `//go:build !linux && !windows && !darwin`：不查询真实文件系统，
// 忽略路径参数并返回 `math.MaxInt64`（此处为 `i64::MAX as u64`）。

// 本文件由 pkg/util/sys/storage/sys_other.go 迁移而来，保留 Go 实现结构。
// 该实现对应非 Linux、非 Windows、非 Darwin 平台的目录容量兜底实现。
// 该导入只用于 math.MaxInt64；使用 i64::MAX as u64 表达同一个常量值。

// 对应 Go 的 `//go:build !linux && !windows && !darwin` 构建约束。
// Rust 中 Darwin 通常映射为 macOS 目标，因此这里用 target_os = "macos" 表达原 build tag 的平台排除。
#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "windows"),
    not(target_os = "macos")
))]
#[allow(non_snake_case)]
// GetTargetDirectoryCapacity get the capacity (bytes) of directory
// GetTargetDirectoryCapacity 返回目录容量的兜底值。
// Go 版本忽略 path 参数并返回 math.MaxInt64, nil；这里保留参数和 Result 形状以对应原错误返回通道。
/// 返回目录容量兜底值（`i64::MAX as u64`），忽略实际路径。
pub fn GetTargetDirectoryCapacity<P: AsRef<std::path::Path>>(
    path: P,
) -> Result<u64, std::io::Error> {
    // 原 Go 参数 path 在该平台兜底实现中不会被使用；显式丢弃以标明这是刻意保留的接口形状。
    let _ = path;
    // 对应 `return math.MaxInt64, nil`：没有错误分支，也不查询真实文件系统容量。
    Ok(i64::MAX as u64)
}

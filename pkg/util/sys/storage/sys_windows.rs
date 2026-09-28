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

// Windows 平台目录可用容量查询。
//
// 通过 `GetDiskFreeSpaceExW` 读取调用者在目标目录所在文件系统上的可用空闲字节数；
// 对应 Go `GetTargetDirectoryCapacity`（`//go:build windows`）。

// 本文件由 pkg/util/sys/storage/sys_windows.go 迁移而来，保留 Go 实现结构。
// 该实现对应 Windows 平台上的目录容量查询：通过 Windows GetDiskFreeSpaceEx API 读取目标目录所在
// 文件系统中调用者可用的空闲字节数，然后按 Go 函数原样返回。
// 系统调用封装；Rust 使用微软维护的 windows-sys crate 提供对应绑定。
//
// Go build tag:
// - //go:build windows
//
// Rust 使用 windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW 对应
// windows.GetDiskFreeSpaceEx。

// 对应 Go 的 `//go:build windows`；Rust 里用目标 OS cfg 记录相同的平台意图。
#![cfg(target_os = "windows")]
#![allow(dead_code)]
#![allow(non_snake_case)]

use std::os::windows::ffi::OsStrExt;

// GetTargetDirectoryCapacity get the capacity (bytes) of directory
// GetTargetDirectoryCapacity 对应 Go 函数 `GetTargetDirectoryCapacity(path string) (uint64, error)`。
// 参数 path 是待查询目录路径；返回值保留 Go 的“可用容量字节数 + error”语义，用 Result 表达错误分支。
/// 查询 `path` 所在磁盘上调用者可用的空闲字节数；失败时返回 Windows OS 错误。
pub fn GetTargetDirectoryCapacity<P: AsRef<std::path::Path>>(
    path: P,
) -> Result<u64, std::io::Error> {
    // 对应 Go 的 `var freeBytes uint64`，该变量会由 Windows API 写入“调用者可用空闲字节数”。
    let mut free_bytes: u64 = 0;

    // Go 通过 windows.StringToUTF16Ptr(path) 把 UTF-8 字符串转成以 NUL 结尾的 UTF-16 指针。
    // Rust FFI 需要显式保留 UTF-16 缓冲区，确保系统调用期间指针仍然有效。
    let wide_path: Vec<u16> = path
        .as_ref()
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // 对应 Go 的 `windows.GetDiskFreeSpaceEx(..., &freeBytes, nil, nil)`。
    // 后两个 nil 表示不读取总容量和总空闲容量；这里只迁移原 Go 代码实际使用的第一个输出参数。
    // 这是 Windows 系统 IO 边界：API 会读取路径指针并写入 free_bytes，因此 Rust 调用需要 unsafe。
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide_path.as_ptr(),
            &mut free_bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        // 保留 Go 的错误返回形状：系统调用失败时返回 0 和原始 Windows OS error。
        return Err(std::io::Error::last_os_error());
    }

    // 对应 Go 的 `return freeBytes, nil`，表示系统调用成功且容量值已经写入。
    Ok(free_bytes)
}

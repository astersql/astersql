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

// POSIX（linux/darwin）目录可用容量查询。
//
// 通过 `statfs` 读取目标目录所在文件系统的可用块数（Bavail）与块大小（Bsize），
// 相乘得到非特权用户可用容量字节数；对应 Go `GetTargetDirectoryCapacity`。

// 本文件由 pkg/util/sys/storage/sys_posix.go 迁移而来，保留 Go 实现结构。
// 该实现对应 linux 或 darwin 平台上的目录容量查询：通过 POSIX statfs 读取目标目录所在文件系统
// 的可用块数和块大小，然后返回可用容量字节数。
//
// Go build tag:
// - //go:build linux || darwin
//
// Rust 使用成熟的 libc crate 提供 statfs 绑定。

// 对应 Go 的 `//go:build linux || darwin`；Rust 里用目标 OS cfg 记录相同的平台意图。
#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(dead_code)]
#![allow(non_snake_case)]

// GetTargetDirectoryCapacity get the capacity (bytes) of directory
// GetTargetDirectoryCapacity 对应 Go 函数 `GetTargetDirectoryCapacity(path string) (uint64, error)`。
// 参数 path 是待查询目录路径；返回值保留 Go 的“容量字节数 + error”语义，用 Result 表达错误分支。
/// 查询 `path` 所在文件系统的可用容量（字节）；失败时返回 OS 错误。
pub fn GetTargetDirectoryCapacity<P: AsRef<std::path::Path>>(
    path: P,
) -> Result<u64, std::io::Error> {
    use std::os::unix::ffi::OsStrExt;

    // Go 直接把 string 传给 syscall.Statfs；Rust FFI 需要先转为以 NUL 结尾的 C 字符串。
    // 如果 path 内含 NUL 字节，Go 调用会由 syscall 层返回错误；这里转成 InvalidInput。
    let c_path = std::ffi::CString::new(path.as_ref().as_os_str().as_bytes())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;

    // 对应 Go 的 `var stat syscall.Statfs_t`。
    // libc 结构体由系统调用完整写入，因此这里先零初始化。
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };

    // 对应 `err := syscall.Statfs(path, &stat)`。
    // 这是系统 IO 边界：libc 会读取路径并写入 stat 结构体，所以 Rust 调用需要 unsafe。
    let err = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if err != 0 {
        // 保留 Go 的错误返回形状：Statfs 失败时返回 0 和原始系统错误。
        return Err(std::io::Error::last_os_error());
    }

    // 对应 Go 的 `c := stat.Bavail * uint64(stat.Bsize)`。
    // Bavail 表示非特权用户可用块数，Bsize 表示块大小；二者相乘得到可用容量字节数。
    let bavail = stat.f_bavail as u64;
    let bsize = stat.f_bsize as u64;
    // Go 的 uint64 乘法在溢出时按无符号整数包裹；这里用 wrapping_mul 保留该算术语义。
    let c = bavail.wrapping_mul(bsize);

    // 对应 Go 的 `return c, nil`，表示系统调用成功且容量计算完成。
    Ok(c)
}

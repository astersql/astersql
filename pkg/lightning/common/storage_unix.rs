// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Unix/类 Unix 平台的磁盘容量探测与同盘判断。
//
// Lightning 导入前需要确认排序目录等路径的可用空间，以及两个目录是否位于同一块磁盘，
// 以便决定是否可安全共享临时文件。本文件通过文件系统系统调用与 inode 设备号实现。

use crate::{CommonError, StorageSize};
use std::ffi::CString;
use std::mem::MaybeUninit;

/// 查询 `dir` 所在文件系统的总容量与可用空间（字节）。
///
/// 直接调用 POSIX `statvfs`，使用文件系统的基本块大小计算容量，避免依赖
/// 外部 `df` 命令、`PATH` 或本地化文本输出。
pub fn GetStorageSize(dir: &str) -> Result<StorageSize, CommonError> {
    let path = CString::new(dir).map_err(|error| {
        CommonError::new(
            "storage",
            format!("cannot get disk capacity at {dir}: {error}"),
        )
    })?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stat` points to writable storage which is
    // only assumed initialized after a successful `statvfs` return.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(CommonError::new(
            "storage",
            format!(
                "cannot get disk capacity at {dir}: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    // SAFETY: a zero return from `statvfs` initializes the output structure.
    let stat = unsafe { stat.assume_init() };
    let block_size = stat.f_frsize as u64;
    Ok(StorageSize {
        Capacity: (stat.f_blocks as u64).wrapping_mul(block_size),
        Available: (stat.f_bavail as u64).wrapping_mul(block_size),
    })
}

/// 判断两个目录是否位于同一块物理/逻辑磁盘（同一设备号）。
///
/// 比较 `stat` 元数据中的 `dev()`；同盘时可避免跨盘硬链接或错误的空间估算。
pub fn SameDisk(dir1: &str, dir2: &str) -> Result<bool, CommonError> {
    use std::os::unix::fs::MetadataExt;
    let first =
        std::fs::metadata(dir1).map_err(|error| CommonError::new("storage", error.to_string()))?;
    let second =
        std::fs::metadata(dir2).map_err(|error| CommonError::new("storage", error.to_string()))?;
    Ok(first.dev() == second.dev())
}

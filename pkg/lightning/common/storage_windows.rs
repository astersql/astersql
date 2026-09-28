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

// Windows 平台的磁盘容量探测与同盘判断。
//
// 与 Unix 版职责相同：为 Lightning 提供目标路径的总容量/可用空间。
// 直接调用 Win32 `GetDiskFreeSpaceExW`，与 Go 实现保持一致。

use crate::{CommonError, StorageSize};
use std::io;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDiskFreeSpaceExW(
        directory_name: *const u16,
        free_bytes_available: *mut u64,
        total_number_of_bytes: *mut u64,
        total_number_of_free_bytes: *mut u64,
    ) -> i32;
}

pub(crate) fn GetStorageSizeWith<F>(dir: &str, query: F) -> Result<StorageSize, CommonError>
where
    F: FnOnce(&str, &mut u64, &mut u64) -> io::Result<()>,
{
    let mut size = StorageSize::default();
    query(dir, &mut size.Available, &mut size.Capacity).map_err(|error| {
        CommonError::new(
            "storage",
            format!("cannot get disk capacity at {dir}: {error}"),
        )
    })?;
    Ok(size)
}

/// 查询 `dir` 所在驱动器的总容量与调用者可用空间（字节）。
#[cfg(windows)]
pub fn GetStorageSize(dir: &str) -> Result<StorageSize, CommonError> {
    use std::os::windows::ffi::OsStrExt;

    GetStorageSizeWith(dir, |path, available, capacity| {
        let path: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `path` is NUL-terminated and lives through the call; both output
        // pointers refer to initialized writable `u64`s, and the final optional
        // output is intentionally null just like the Go implementation.
        let result = unsafe {
            GetDiskFreeSpaceExW(path.as_ptr(), available, capacity, std::ptr::null_mut())
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

#[cfg(not(windows))]
pub fn GetStorageSize(dir: &str) -> Result<StorageSize, CommonError> {
    GetStorageSizeWith(dir, |_, _, _| {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows storage API is unavailable on this platform",
        ))
    })
}

/// Windows 上同盘判断的占位实现：始终返回 `false`。
///
/// Keep the Go implementation's explicit FIXME behavior on Windows.
pub fn SameDisk(_dir1: &str, _dir2: &str) -> Result<bool, CommonError> {
    // Keep the Go implementation's explicit FIXME behavior on Windows.
    Ok(false)
}

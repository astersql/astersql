// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 本地磁盘容量与同盘判定的平台分发入口。
//
// Lightning 导入前常需检查目标路径剩余空间，以及排序临时目录与数据目录是否在同一物理盘，
// 以避免跨盘 IO 影响性能。具体实现分别在 `storage_unix` / `storage_windows`。

use crate::CommonError;

/// 存储容量快照：总容量与可用空间（字节）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageSize {
    /// 文件系统总容量（字节）。
    pub Capacity: u64,
    /// 当前可用空间（字节）。
    pub Available: u64,
}

/// 查询 `path` 所在文件系统的容量与可用空间；按目标平台调用 unix/windows 实现。
pub fn GetStorageSize(path: &str) -> Result<StorageSize, CommonError> {
    #[cfg(unix)]
    {
        crate::storage_unix::GetStorageSize(path)
    }
    #[cfg(windows)]
    {
        crate::storage_windows::GetStorageSize(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(CommonError::new("storage", "unsupported platform"))
    }
}

/// 判断两个路径是否位于同一磁盘/文件系统；非 unix/windows 平台直接返回 `false`。
pub fn SameDisk(left: &str, right: &str) -> Result<bool, CommonError> {
    #[cfg(unix)]
    {
        crate::storage_unix::SameDisk(left, right)
    }
    #[cfg(windows)]
    {
        crate::storage_windows::SameDisk(left, right)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(false)
    }
}

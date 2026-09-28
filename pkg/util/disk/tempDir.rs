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

// 临时存储目录生命周期管理。
//
// 对应 Go `tempDir.go`：按全局配置路径创建/加锁临时目录，异步清理陈旧条目，
// 退出时释放文件锁。执行器 spill（内存不足时把中间结果落到磁盘）依赖此目录。

use crate::config::get_global_config;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

/// 持有临时目录排他文件锁，进程退出前由 `CleanUp` 释放。
static tempDirLock: Mutex<Option<File>> = Mutex::new(None);
/// 模拟 Go singleflight：同一时刻仅允许一路初始化。
static sf: Mutex<()> = Mutex::new(());

/// 目录锁文件名，初始化与清理时均保留。
const lockFile: &str = "_dir.lock";
/// 需长期保留的记录子目录名。
const recordDir: &str = "record";

/// 构造互斥锁中毒时的 I/O 错误。
fn poisoned_lock(name: &str) -> io::Error {
    io::Error::other(format!("{name} lock poisoned"))
}

// CheckAndInitTempDir checks whether the temp directory exists and initializes
// it if needed. The mutex preserves the Go singleflight behavior for callers:
// only one initialization for the shared key can run at a time.
/// 若临时目录尚不存在则初始化；并发调用串行化，避免重复初始化。
pub fn CheckAndInitTempDir() -> io::Result<()> {
    let _flight = sf
        .lock()
        .map_err(|_| poisoned_lock("temp directory init"))?;
    if !checkTempDirExist() {
        InitializeTempDir()?;
    }
    Ok(())
}

/// 通过 metadata 探测配置的临时存储路径是否已存在。
pub(crate) fn checkTempDirExist() -> bool {
    fs::metadata(&get_global_config().temp_storage_path).is_ok()
}

// InitializeTempDir creates and locks the configured directory, then removes
// stale entries asynchronously while preserving the lock and record entries.
/// 创建并排他锁定配置目录，条目超过 2 个时异步删除非锁/非 record 的陈旧内容。
pub fn InitializeTempDir() -> io::Result<()> {
    let temp_dir = PathBuf::from(&get_global_config().temp_storage_path);
    // Match Go's `os.Stat`/`os.MkdirAll` sequence: an existing non-directory is
    // left for opening `<path>/_dir.lock` to report `NotADirectory`.
    if fs::metadata(&temp_dir).is_err() {
        create_dir_all(&temp_dir)?;
    }

    // 打开 `_dir.lock` 并 try_lock_exclusive，阻止多实例共用同一临时根。
    let lock_path = temp_dir.join(lockFile);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock_exclusive()?;
    *tempDirLock
        .lock()
        .map_err(|_| poisoned_lock("temp directory file"))? = Some(lock);

    let sub_dirs = fs::read_dir(&temp_dir)?.collect::<io::Result<Vec<_>>>()?;
    // 目录项多于 lock+record 时，后台线程清理其余陈旧文件/子目录。
    if sub_dirs.len() > 2 {
        thread::spawn(move || {
            for sub_dir in sub_dirs {
                let name = sub_dir.file_name();
                if name == lockFile || name == recordDir {
                    continue;
                }
                let path = temp_dir.join(name);
                let result = if path.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                if let Err(err) = result {
                    eprintln!("remove temporary entry {}: {err}", path.display());
                }
            }
        });
    }
    Ok(())
}

// CleanUp releases the directory lock when exiting TiDB.
/// 进程退出时释放临时目录文件锁。
pub fn CleanUp() {
    let Ok(mut guard) = tempDirLock.lock() else {
        return;
    };
    if let Some(lock) = guard.take() {
        if let Err(err) = lock.unlock() {
            eprintln!("release temporary directory lock: {err}");
        }
    }
}

// CheckAndCreateDir checks whether a directory exists and creates it if not.
/// 目录不存在则递归创建（Unix 模式 0750）。
pub fn CheckAndCreateDir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if fs::metadata(path).is_ok() {
        return Ok(());
    }
    create_dir_all(path)
}

/// Unix：递归创建目录，权限 0750。
#[cfg(unix)]
fn create_dir_all(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o750).create(path)
}

/// 非 Unix：使用标准 `create_dir_all`。
#[cfg(not(unix))]
fn create_dir_all(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

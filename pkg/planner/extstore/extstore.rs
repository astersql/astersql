// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 规划器外部存储（external storage）全局单例与本地探测路径。
//
// 外部存储用于 Plan Replayer、云备份等把数据写到 S3/本地文件等后端。
// Classic 内核或未配置云 URI 时回退到日志目录或临时目录下的 `file://` 本地存储。
// keyspace 是多租户命名空间，会拼到对象路径中。

#![allow(dead_code, non_snake_case)]

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context as _, Result};
use log::{info, warn};
use path_clean::PathClean;
use url::Url;

#[cfg(test)]
use super::root_feature_kerneltype as kerneltype;
#[cfg(test)]
pub use astersql_planner_extstore::{config, objstore, vardef};

pub use objstore::storage::{Context, Storage, StorageRef};

/// 进程级全局外部存储句柄（惰性初始化）。
static globalExtStorage: Mutex<Option<StorageRef>> = Mutex::new(None);
/// 测试注入的本地路径探测文件系统（对应 Go afero.Fs）。
static testLocalPathFS: Mutex<Option<Arc<dyn ProbeFileSystem>>> = Mutex::new(None);

/// 加锁；若 mutex 被 poison 则取回内部值，避免测试崩溃传染。
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A writable file used by the same open/write/close probe as Go's afero file.
/// 可写探测文件：与 Go afero 相同的 open/write/close 探测接口。
pub trait ProbeFile: Write + Send {
    /// 关闭文件（消费 `Box<Self>`）。
    fn close(self: Box<Self>) -> io::Result<()>;
}

/// The narrow filesystem boundary injected by the Go tests through afero.Fs.
/// 测试可注入的窄文件系统边界（对应 Go afero.Fs）。
pub trait ProbeFileSystem: Send + Sync {
    /// 以读写方式打开（或创建）路径上的文件。
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn ProbeFile>>;
    /// 删除路径上的文件。
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

/// 基于真实 OS 文件的探测文件包装。
struct OsProbeFile(Option<File>);

impl Write for OsProbeFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .as_mut()
            .expect("probe file already closed")
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.as_mut().expect("probe file already closed").flush()
    }
}

impl ProbeFile for OsProbeFile {
    fn close(mut self: Box<Self>) -> io::Result<()> {
        drop(self.0.take());
        Ok(())
    }
}

/// 默认使用本机文件系统的探测实现。
struct OsProbeFileSystem;

impl ProbeFileSystem for OsProbeFileSystem {
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn ProbeFile>> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map(|file| Box::new(OsProbeFile(Some(file))) as Box<dyn ProbeFile>)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
}

/// GetGlobalExtStorage returns the lazily initialized global external storage.
/// 返回惰性初始化的全局外部存储引用。
pub fn GetGlobalExtStorage(ctx: &Context) -> Result<StorageRef> {
    let mut storage = lock_unpoisoned(&globalExtStorage);
    if storage.is_none() {
        *storage = Some(createGlobalExtStorage(ctx)?);
    }
    Ok(Arc::clone(
        storage.as_ref().expect("global storage initialized"),
    ))
}

/// 按内核类型与云 URI 配置创建全局外部存储；失败时记录脱敏 URI。
fn createGlobalExtStorage(ctx: &Context) -> Result<StorageRef> {
    let keyspaceName = config::get_global_keyspace_name();
    let mut uri = vardef::CloudStorageURI.Load();

    // Classic 或未配置云 URI：回退到可写本地目录的 file://。
    if kerneltype::IsClassic() || uri.is_empty() {
        let mut localPath = getLocalPathDirName();
        // Go filepath.Abs is lexical: it also succeeds for paths that do not
        // exist and does not resolve symlinks as canonicalize would.
        if localPath.is_absolute() {
            localPath = localPath.clean();
        } else if let Ok(current_dir) = std::env::current_dir() {
            localPath = current_dir.join(localPath).clean();
        }
        info!(
            "using default local storage: localPath={}, keyspaceName={}",
            localPath.display(),
            keyspaceName
        );
        uri = format!("file://{}", localPath.display());
    }

    match NewExtStorage(ctx, &uri, &keyspaceName) {
        Ok(storage) => {
            info!("initialized global ext storage: storage={}", storage.URI());
            Ok(storage)
        }
        Err(error) => {
            warn!(
                "failed to create global ext storage: uri={}, error={error:#}",
                redact_url(&uri)
            );
            Err(error)
        }
    }
}

/// Sets or clears the global storage. Like Go, this entry point is for tests.
/// 设置或清空全局存储（测试入口，对齐 Go）。
pub fn SetGlobalExtStorageForTest(storage: Option<StorageRef>) {
    *lock_unpoisoned(&globalExtStorage) = storage;
}

/// Installs the afero-equivalent filesystem boundary used by focused tests.
/// 安装测试用的本地路径探测文件系统。
pub fn SetLocalPathFileSystemForTest(fs: Option<Arc<dyn ProbeFileSystem>>) {
    *lock_unpoisoned(&testLocalPathFS) = fs;
}

/// NewExtStorage parses the URL, appends the keyspace namespace, and creates the real backend.
/// 解析 URL、追加 keyspace 命名空间并创建真实后端。
pub fn NewExtStorage(ctx: &Context, rawURL: &str, namespace: &str) -> Result<StorageRef> {
    let mut url = objstore::parse::ParseRawURL(rawURL)
        .with_context(|| format!("parse external storage URL {}", redact_url(rawURL)))?;
    if !namespace.is_empty() {
        // Go filepath.Join keeps the existing URL path when a later element
        // starts with a separator; Path::join would otherwise replace it.
        let relative_namespace = Path::new(namespace)
            .components()
            .filter(|component| !matches!(component, Component::Prefix(_) | Component::RootDir))
            .collect::<PathBuf>();
        url.path = Path::new(&url.path)
            .join(relative_namespace)
            .clean()
            .to_string_lossy()
            .into_owned();
    }

    let backend = objstore::parse::ParseBackendFromURL(&mut url, None)
        .with_context(|| format!("parse external storage backend {}", redact_url(rawURL)))?;
    objstore::storage::New(ctx, &backend, None)
        .with_context(|| format!("create external storage {}", redact_url(rawURL)))
}

/// 选择本地外部存储根目录：优先日志目录（可写探测通过），否则用临时目录。
fn getLocalPathDirName() -> PathBuf {
    let fs = lock_unpoisoned(&testLocalPathFS)
        .as_ref()
        .cloned()
        .unwrap_or_else(|| Arc::new(OsProbeFileSystem));
    let global = config::get_global_config();
    let logFile = Path::new(&global.log.file.filename);
    let parent = logFile.parent().unwrap_or_else(|| Path::new("."));
    let tidbLogDir = if parent.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        parent.clean()
    };

    if canWriteToReplayerDirFile(fs.as_ref(), &tidbLogDir) {
        info!("use log dir as local path: dir={}", tidbLogDir.display());
        return tidbLogDir;
    }

    let tempDir = PathBuf::from(&global.temp_dir);
    info!("use temp dir as local path: dir={}", tempDir.display());
    tempDir
}

/// 在 `dir/replayer/test_<timestamp>.txt` 上做写探测，判断目录是否可用。
fn canWriteToReplayerDirFile(vfs: &dyn ProbeFileSystem, dir: &Path) -> bool {
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    let path = dir.join("replayer").join(format!("test_{timestamp}.txt"));
    if !canWriteToFileInternal(vfs, &path) {
        warn!("cannot write to file: path={}", path.display());
        return false;
    }
    true
}

/// 打开、写一字节、关闭并尽量删除探测文件；任一步失败则不可写。
fn canWriteToFileInternal(vfs: &dyn ProbeFileSystem, path: &Path) -> bool {
    let mut file = match vfs.open_file(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let write_ok = file.write(&[0]).is_ok();
    let close_ok = file.close().is_ok();
    if close_ok {
        if let Err(error) = vfs.remove_file(path) {
            warn!("failed to delete probe file {}: {error}", path.display());
        }
    } else {
        warn!("failed to close probe file {}", path.display());
    }
    write_ok
}

/// 脱敏 URL 查询参数中的云凭证（access-key、account-key 等），避免日志泄露。
fn redact_url(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return raw.to_owned();
    };
    let redact_keys: &[&str] = match url.scheme().to_ascii_lowercase().as_str() {
        "s3" | "ks3" | "oss" => &["access-key", "secret-access-key", "session-token"],
        "azure" | "azblob" => &["account-key", "encryption-key", "sas-token"],
        _ => &[],
    };
    if redact_keys.is_empty() {
        return url.into();
    }

    // 重建 query：敏感键替换为 xxxxxx，其余保留。
    let pairs = url
        .query_pairs()
        .map(|(key, value)| {
            let normalized = key.replace('_', "-").to_ascii_lowercase();
            let value = if redact_keys.contains(&normalized.as_str()) {
                "xxxxxx".to_owned()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect::<Vec<_>>();
    url.query_pairs_mut().clear().extend_pairs(pairs);
    url.into()
}

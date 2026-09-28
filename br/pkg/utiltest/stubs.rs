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

//! Local stand-ins for `objstore` / `storeapi` used by [`crate::suite`].
//!
//! Darwin arm64-safe: no kv/domain/kvproto/grpcio/objstore crate edge.
//! 本地对象存储桩：供 RestoreSchemaSuite 使用，无真实 objstore 依赖。
//! LocalStorage 对齐 Go NewLocalStorage；Close 不会禁用后续 IO。
//! Context 为无取消空壳；WalkOption 支持 ObjPrefix/SkipSubDir。
//! WriteFile 经临时文件 rename 提交；Delete 默认传播缺失文件错误。
//! 本文件是适配边界，不实现云厂商预签名真实逻辑。

use std::fs;
// Read/Write 用于文件流。
use std::io::{Read, Seek, SeekFrom, Write as IoWrite};
// Path/PathBuf 路径拼接。
use std::path::{Path, PathBuf};
// Arc 包装 Storage trait 对象。
use std::sync::Arc;
// Presign 过期占位参数。
use std::time::Duration;

#[derive(Clone, Debug)]
/// 存储错误：message + is_not_exist 标志。
pub struct Error {
    /// 错误文案。
    pub message: String,
    /// 是否对应文件不存在。
    pub is_not_exist: bool,
}

impl Error {
    /// 普通错误构造。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            is_not_exist: false,
        }
    }

    /// not-exist 错误构造。
    pub fn not_exist(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            is_not_exist: true,
        }
    }
}

// Display 仅输出 message。
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// 接入标准 Error trait。
impl std::error::Error for Error {}

// 本包 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// Stand-in for `objectio::Context` / Go `context.Context`.
#[derive(Clone, Default)]
/// Context 空壳替身；background 返回自身。
pub struct Context;

impl Context {
    /// 返回默认上下文。
    pub fn background() -> Self {
        Self
    }
}

#[derive(Clone, Debug, Default)]
/// 遍历选项：子目录与前缀过滤。
pub struct WalkOption {
    /// 遍历子目录。
    pub SubDir: String,
    /// 对象名前缀过滤。
    pub ObjPrefix: String,
    /// 为 true 时不递归子目录。
    pub SkipSubDir: bool,
}

#[derive(Clone, Debug, Default)]
/// 读选项：起点包含、终点不包含。
pub struct ReaderOption {
    pub StartOffset: Option<i64>,
    pub EndOffset: Option<i64>,
    pub PrefetchSize: i32,
}

#[derive(Clone, Debug, Default)]
/// 写选项占位。
pub struct WriterOption;

/// 流式读接口。
pub trait Reader: Send {
    /// 读入缓冲区。
    fn Read(&mut self, ctx: &Context, buf: &mut [u8]) -> Result<usize>;
    /// 关闭流。
    fn Close(&mut self, ctx: &Context) -> Result<()>;
}

/// 流式写接口。
pub trait Writer: Send {
    /// 写入字节。
    fn Write(&mut self, ctx: &Context, p: &[u8]) -> Result<usize>;
    fn Close(&mut self, ctx: &Context) -> Result<()>;
}

/// Matches Go `storeapi.Storage` surface used by restore-schema suites.
/// 对象存储表面，对齐 suite 所需子集。
pub trait Storage: Send + Sync {
    /// 写整文件。
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()>;
    /// 读整文件。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>>;
    /// 判断存在。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool>;
    /// 删单文件。
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()>;
    /// 批量删除。
    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()>;
    /// 遍历目录并回调。
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;
    /// 返回 local:// URI。
    fn URI(&self) -> String;
    /// 打开读流。
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        _option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>>;
    /// 创建写流（临时文件）。
    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>>;
    /// 重命名对象。
    fn Rename(&self, ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()>;
    /// 伪预签名 URL。
    fn PresignFile(&self, _ctx: &Context, file_name: &str, _expire: Duration) -> Result<String>;
    /// 关闭存储。
    fn Close(&self);
}

/// Local filesystem storage matching Go `objstore.NewLocalStorage`.
/// 本地目录存储实现。
pub struct LocalStorage {
    // 根目录路径。
    root: PathBuf,
}

impl LocalStorage {
    /// 创建根目录并构造。
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|e| Error::new(format!("mkdir {}: {e}", root.display())))?;
        Ok(Self { root })
    }

    // 逻辑名转绝对路径。
    fn full_path(&self, name: &str) -> PathBuf {
        let mut p = self.root.clone();
        for part in name.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p
    }

    // Go LocalStorage.Close is a no-op, so every operation remains available.
    fn ensure_open(&self) -> Result<()> {
        Ok(())
    }
}

// 磁盘读句柄。
struct FileReader {
    file: Option<fs::File>,
    pos: i64,
    end_pos: Option<i64>,
}

impl Reader for FileReader {
    fn Read(&mut self, _ctx: &Context, buf: &mut [u8]) -> Result<usize> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| Error::new("reader closed"))?;
        let allowed = self
            .end_pos
            .map(|end| (end - self.pos).max(0) as usize)
            .unwrap_or(buf.len())
            .min(buf.len());
        if allowed == 0 {
            return Ok(0);
        }
        let read = file
            .read(&mut buf[..allowed])
            .map_err(|e| Error::new(format!("read: {e}")))?;
        self.pos += read as i64;
        Ok(read)
    }
    fn Close(&mut self, _ctx: &Context) -> Result<()> {
        self.file.take();
        Ok(())
    }
}

// 目标文件的带缓冲写句柄，Close 时 flush 并关闭。
struct FileWriter {
    file: Option<std::io::BufWriter<fs::File>>,
}

impl Writer for FileWriter {
    fn Write(&mut self, _ctx: &Context, p: &[u8]) -> Result<usize> {
        self.file
            .as_mut()
            .ok_or_else(|| Error::new("writer closed"))?
            .write(p)
            .map_err(|e| Error::new(format!("write: {e}")))
    }
    fn Close(&mut self, _ctx: &Context) -> Result<()> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| Error::new("writer closed"))?;
        file.flush().map_err(|e| Error::new(format!("flush: {e}")))
    }
}

// LocalStorage：所有方法先 ensure_open。
impl Storage for LocalStorage {
    fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        // 关闭后拒绝操作。
        self.ensure_open()?;
        let path = self.full_path(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("mkdir parent {}: {e}", parent.display())))?;
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)
            .map_err(|e| Error::new(format!("create temp in {}: {e}", parent.display())))?;
        tmp.write_all(data)
            .map_err(|e| Error::new(format!("write temp for {}: {e}", path.display())))?;
        tmp.persist(&path)
            .map_err(|e| Error::new(format!("rename temp to {}: {}", path.display(), e.error)))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).map_err(|e| {
                Error::new(format!("chmod {} after atomic write: {e}", path.display()))
            })?;
        }
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
        self.ensure_open()?;
        let path = self.full_path(name);
        fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                // NotFound 映射为 is_not_exist。
                Error::not_exist(format!("read {}: {e}", path.display()))
            } else {
                Error::new(format!("read {}: {e}", path.display()))
            }
        })
    }

    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
        self.ensure_open()?;
        Ok(self.full_path(name).exists())
    }

    fn DeleteFile(&self, _ctx: &Context, name: &str) -> Result<()> {
        self.ensure_open()?;
        let path = self.full_path(name);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::not_exist(format!("delete {}: {e}", path.display())))
            }
            Err(e) => Err(Error::new(format!("delete {}: {e}", path.display()))),
        }
    }

    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()> {
        for name in names {
            self.DeleteFile(ctx, name)?;
        }
        Ok(())
    }

    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        self.ensure_open()?;
        let base = if opt.SubDir.is_empty() {
            self.root.clone()
        } else {
            self.full_path(&opt.SubDir)
        };
        // 目录不存在则空遍历成功。
        if !base.exists() {
            return Ok(());
        }
        walk_dir_recursive(
            &self.root,
            &base,
            &base,
            &opt.ObjPrefix,
            opt.SkipSubDir,
            fn_,
        )
    }

    fn URI(&self) -> String {
        // URI 含根绝对路径。
        format!("file://{}", self.root.display())
    }

    fn Open(
        &self,
        _ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>> {
        self.ensure_open()?;
        let path = self.full_path(name);
        let mut file = fs::File::open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::not_exist(format!("open {}: {e}", path.display()))
            } else {
                Error::new(format!("open {}: {e}", path.display()))
            }
        })?;
        let start = option.and_then(|o| o.StartOffset).unwrap_or(0);
        if start < 0 {
            return Err(Error::new(format!("invalid start offset: {start}")));
        }
        if start != 0 {
            file.seek(SeekFrom::Start(start as u64))
                .map_err(|e| Error::new(format!("seek {}: {e}", path.display())))?;
        }
        Ok(Box::new(FileReader {
            file: Some(file),
            pos: start,
            end_pos: option.and_then(|o| o.EndOffset),
        }))
    }

    fn Create(
        &self,
        _ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>> {
        self.ensure_open()?;
        let path = self.full_path(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("mkdir parent {}: {e}", parent.display())))?;
        }
        let file = fs::File::create(&path)
            .map_err(|e| Error::new(format!("create {}: {e}", path.display())))?;
        Ok(Box::new(FileWriter {
            file: Some(std::io::BufWriter::new(file)),
        }))
    }

    fn Rename(&self, _ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()> {
        self.ensure_open()?;
        let old = self.full_path(old_file_name);
        let new = self.full_path(new_file_name);
        if let Some(parent) = new.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::new(format!("mkdir parent {}: {e}", parent.display())))?;
        }
        fs::rename(&old, &new).map_err(|e| {
            Error::new(format!(
                "rename {} -> {}: {e}",
                old.display(),
                new.display()
            ))
        })
    }

    fn PresignFile(&self, _ctx: &Context, file_name: &str, _expire: Duration) -> Result<String> {
        Ok(Path::new(file_name)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned())
    }

    fn Close(&self) {
        // Go LocalStorage owns no closeable shared resource.
    }
}

// 递归遍历；支持前缀过滤与跳过子目录。
fn walk_dir_recursive(
    root: &Path,
    walk_base: &Path,
    current: &Path,
    obj_prefix: &str,
    skip_sub_dir: bool,
    fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
) -> Result<()> {
    let entries = fs::read_dir(current)
        .map_err(|e| Error::new(format!("readdir {}: {e}", current.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::new(format!("readdir entry: {e}")))?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            // 路径分隔符归一化。
            .replace('\\', "/");
        let meta = entry
            .metadata()
            .map_err(|e| Error::new(format!("metadata {}: {e}", path.display())))?;
        if meta.is_dir() {
            // SkipSubDir：不进入子目录。
            if skip_sub_dir {
                continue;
            }
            walk_dir_recursive(root, walk_base, &path, obj_prefix, skip_sub_dir, fn_)?;
            continue;
        }
        let relative_to_walk_base = path
            .strip_prefix(walk_base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if !obj_prefix.is_empty() && !relative_to_walk_base.starts_with(obj_prefix) {
            continue;
        }
        fn_(&rel, meta.len() as i64)?;
    }
    Ok(())
}

/// Matches Go `objstore.NewLocalStorage(base)`.
/// 工厂：返回 Arc<dyn Storage>，对齐 Go 签名。
pub fn NewLocalStorage(base: impl AsRef<Path>) -> Result<Arc<dyn Storage>> {
    Ok(Arc::new(LocalStorage::new(base.as_ref())?))
}

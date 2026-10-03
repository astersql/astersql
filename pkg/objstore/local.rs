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

// 本地文件系统对象存储（`file://`）。
//
// 将对象名映射到根目录下的普通文件，支持原子写（临时文件 + rename）、
// 范围读、目录遍历、硬链接复制等，对应 Go `local.go`。

use std::any::Any;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use uuid::Uuid;
use walkdir::WalkDir;

#[cfg(not(windows))]
use crate::local_unix::mkdirAll;
#[cfg(windows)]
use crate::local_windows::mkdirAll;
use crate::storage::{
    Context, CopySpec, ObjectReader, ObjectWriter, ReaderOption, Storage, StorageRef,
    TombstoneSize, WalkOption, WriterOption,
};
use objectio as objectio_api;
use storeapi as storeapi_api;

/// 本地对象默认文件权限（Unix mode）。
const localFilePerm: u32 = 0o644;
/// 本地存储 URI 前缀。
pub const LocalURIPrefix: &str = "file://";

/// 基于本地目录的对象存储实现（`file://`）。
pub struct LocalStorage {
    /// 对象根目录。
    base: PathBuf,
    /// 删除时忽略“文件不存在”（ENOENT）错误。
    pub IgnoreEnoentForDelete: bool,
}

impl LocalStorage {
    /// 返回根目录路径字符串。
    pub fn Base(&self) -> String {
        self.base.to_string_lossy().into_owned()
    }

    /// 将对象名拼到根目录下。
    fn object_path(&self, name: &str) -> PathBuf {
        // Go filepath.Join keeps the first path as the base even when a later
        // element starts with a separator. PathBuf::join instead replaces the
        // base for absolute paths, so remove only the root/prefix components.
        let relative = Path::new(name)
            .components()
            .filter(|component| !matches!(component, Component::Prefix(_) | Component::RootDir))
            .collect::<PathBuf>();
        self.base.join(relative)
    }
}

impl LocalStorage {
    fn create_buffered(&self, name: &str, part_size: Option<i64>) -> Result<Box<dyn ObjectWriter>> {
        let path = self.object_path(name);
        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
        let file = File::create(path)?;
        let writer = match part_size.filter(|size| *size > 0) {
            Some(size) => BufWriter::with_capacity((size as usize).max(16), file),
            None => BufWriter::new(file),
        };
        Ok(Box::new(LocalWriter {
            writer,
            closed: false,
        }))
    }
}

impl Storage for LocalStorage {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// 删除对象；可选忽略不存在错误。
    fn DeleteFile(&self, _ctx: &Context, name: &str) -> Result<()> {
        match fs::remove_file(self.object_path(name)) {
            Ok(()) => Ok(()),
            Err(error) if self.IgnoreEnoentForDelete && error.kind() == io::ErrorKind::NotFound => {
                Ok(())
            }
            Err(error) => Err(error).with_context(|| format!("failed to delete file {name}")),
        }
    }

    /// 原子写：先写临时文件再 rename；目录不存在时建目录重试。
    fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        let target = self.object_path(name);
        // 临时文件名含 UUID，避免并发写冲突。
        let temporary = PathBuf::from(format!("{}.tmp.{}", target.display(), Uuid::new_v4()));
        if let Err(first_error) = write_file_with_mode(&temporary, data, localFilePerm) {
            let parent = temporary.parent().unwrap_or_else(|| Path::new("."));
            // 首次写入失败时：若父目录不存在则 mkdir 后重试。
            match pathExists(parent) {
                Err(exists_error) => {
                    return Err(first_error).context(format!(
                        "after failed to write file, failed to check path exists : {exists_error}"
                    ));
                }
                Ok(true) => return Err(first_error.into()),
                Ok(false) => {}
            }
            if let Err(mkdir_error) = mkdirAll(parent) {
                return Err(first_error).context(format!(
                    "after failed to write file, failed to mkdir : {mkdir_error}"
                ));
            }
            write_file_with_mode(&temporary, data, localFilePerm)?;
        }
        // rename 保证读者要么看到旧文件要么看到完整新文件。
        fs::rename(temporary, target)?;
        Ok(())
    }

    /// 整文件读取。
    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
        fs::read(self.object_path(name)).map_err(Into::into)
    }

    /// 检查对象是否存在。
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
        pathExists(&self.object_path(name))
    }

    /// 打开对象为可读流，支持起止偏移（范围读）。
    fn Open(
        &self,
        _ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn ObjectReader>> {
        let mut file = File::open(self.object_path(name))?;
        let mut position = 0_i64;
        let mut end_position = -1_i64;
        if let Some(option) = option {
            if let Some(end) = option.end_offset {
                end_position = end;
            }
            // 负 start 非法；有效 start 则 seek 定位。
            if let Some(start) = option.start_offset {
                if start < 0 {
                    return Err(anyhow!("invalid negative start offset: {start}"));
                }
                file.seek(SeekFrom::Start(start as u64))?;
                position = start;
            }
        }
        Ok(Box::new(LocalFile {
            file,
            position,
            end_position,
            closed: false,
        }))
    }

    /// 遍历目录下对象，支持子目录、前缀、start_after、跳过子目录与 tombstone。
    fn WalkDir(
        &self,
        _ctx: &Context,
        option: Option<&WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        let default_option = WalkOption::default();
        let option = option.unwrap_or(&default_option);
        let walk_base = self.object_path(&option.sub_dir);
        // 目标子目录不存在：可选以 tombstone 回调一次后返回。
        if !walk_base.exists() {
            if option.include_tombstone {
                let relative = slash_path(
                    walk_base
                        .strip_prefix(&self.base)
                        .unwrap_or(walk_base.as_path()),
                );
                // Go filepath.Rel(base, base) returns ".", and that value is
                // used for ObjPrefix matching before the callback path is made
                // relative to the storage root.
                let relative_to_walk = ".";
                if relative_to_walk.starts_with(&option.obj_prefix)
                    && (option.start_after.is_empty() || relative > option.start_after)
                {
                    callback(&relative, TombstoneSize)?;
                }
            }
            return Ok(());
        }

        // 按文件名排序遍历，与 Go 侧稳定顺序对齐。
        let mut walker = WalkDir::new(&walk_base).sort_by_file_name();
        if option.skip_sub_dir {
            walker = walker.max_depth(1);
        }
        for entry in walker.into_iter().filter_entry(|entry| {
            if !entry.file_type().is_dir() || entry.path() == walk_base {
                return true;
            }
            let Ok(relative) = entry.path().strip_prefix(&self.base) else {
                return true;
            };
            !should_skip_local_subtree(&slash_path(relative), &option.start_after)
        }) {
            let entry = entry?;
            if entry.file_type().is_dir() {
                continue;
            }
            let relative_to_walk = slash_path(entry.path().strip_prefix(&walk_base)?);
            if !relative_to_walk.starts_with(&option.obj_prefix) {
                continue;
            }
            let relative = slash_path(entry.path().strip_prefix(&self.base)?);
            if !option.start_after.is_empty() && relative <= option.start_after {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            // 软链接：能解析则取目标大小，否则记 0（断裂链接）。
            let size = if metadata.file_type().is_symlink() {
                match fs::metadata(entry.path()) {
                    Ok(metadata) => metadata.len() as i64,
                    Err(_) => 0,
                }
            } else {
                metadata.len() as i64
            };
            callback(&relative, size)?;
        }
        Ok(())
    }

    /// 返回 `file://` + 根路径。
    fn URI(&self) -> String {
        format!("{LocalURIPrefix}{}", self.base.display())
    }

    /// 创建可写对象（直接落盘，非原子临时文件路径）。
    fn Create(
        &self,
        _ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>> {
        self.create_buffered(name, None)
    }

    /// 重命名对象。
    fn Rename(&self, _ctx: &Context, old_name: &str, new_name: &str) -> Result<()> {
        fs::rename(self.object_path(old_name), self.object_path(new_name)).map_err(Into::into)
    }

    /// 本地无预签名：返回文件 basename。
    fn PresignFile(&self, _ctx: &Context, name: &str, _duration: Duration) -> Result<String> {
        Ok(Path::new(name)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned())
    }

    /// 本地存储无连接资源，关闭为空操作。
    fn Close(&self) {}

    /// 仅支持从另一 LocalStorage 硬链接复制。
    fn CopyFrom(&self, _ctx: &Context, source: StorageRef, spec: &CopySpec) -> Result<()> {
        let source = source
            .as_any()
            .downcast_ref::<LocalStorage>()
            .ok_or_else(|| anyhow!("expect source to be LocalStorage"))?;
        let from = source.object_path(&spec.from);
        let to = self.object_path(&spec.to);
        mkdirAll(to.parent().unwrap_or_else(|| Path::new(".")))?;
        fs::hard_link(from, to)?;
        Ok(())
    }

    /// 本地 FS 视为强一致。
    fn is_strong_consistent(&self) -> bool {
        true
    }
}

struct StoreapiLocalReader(Box<dyn ObjectReader>);
impl Read for StoreapiLocalReader {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        self.0.read(data)
    }
}
impl Seek for StoreapiLocalReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}
impl objectio_api::Reader for StoreapiLocalReader {
    fn close(&mut self) -> io::Result<()> {
        self.0.close().map_err(io::Error::other)
    }
    fn file_size(&self) -> io::Result<i64> {
        self.0.get_file_size().map_err(io::Error::other)
    }
}

struct StoreapiLocalWriter(Box<dyn ObjectWriter>);
impl objectio_api::Writer for StoreapiLocalWriter {
    fn write(&mut self, context: &objectio_api::Context, data: &[u8]) -> io::Result<usize> {
        context.check()?;
        self.0
            .write(&Context::default(), data)
            .map_err(io::Error::other)
    }
    fn close(&mut self, context: &objectio_api::Context) -> io::Result<()> {
        context.check()?;
        self.0.close(&Context::default()).map_err(io::Error::other)
    }
}

impl storeapi_api::Storage for LocalStorage {
    fn WriteFile(&self, context: &storeapi_api::Context, name: &str, data: &[u8]) -> Result<()> {
        context.check()?;
        Storage::WriteFile(self, &Context::default(), name, data)
    }
    fn ReadFile(&self, context: &storeapi_api::Context, name: &str) -> Result<Vec<u8>> {
        context.check()?;
        Storage::ReadFile(self, &Context::default(), name)
    }
    fn FileExists(&self, context: &storeapi_api::Context, name: &str) -> Result<bool> {
        context.check()?;
        Storage::FileExists(self, &Context::default(), name)
    }
    fn DeleteFile(&self, context: &storeapi_api::Context, name: &str) -> Result<()> {
        context.check()?;
        Storage::DeleteFile(self, &Context::default(), name)
    }
    fn DeleteFiles(&self, context: &storeapi_api::Context, names: &[String]) -> Result<()> {
        context.check()?;
        Storage::DeleteFiles(self, &Context::default(), names)
    }
    fn Open(
        &self,
        context: &storeapi_api::Context,
        name: &str,
        option: Option<&storeapi_api::ReaderOption>,
    ) -> Result<Box<dyn objectio_api::Reader>> {
        context.check()?;
        let converted = option.map(|value| ReaderOption {
            start_offset: value.StartOffset,
            end_offset: value.EndOffset,
        });
        Storage::Open(self, &Context::default(), name, converted.as_ref())
            .map(|reader| Box::new(StoreapiLocalReader(reader)) as Box<dyn objectio_api::Reader>)
    }
    fn WalkDir(
        &self,
        context: &storeapi_api::Context,
        option: Option<&storeapi_api::WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        context.check()?;
        let converted = option.map(|value| WalkOption {
            sub_dir: value.SubDir.clone(),
            skip_sub_dir: value.SkipSubDir,
            obj_prefix: value.ObjPrefix.clone(),
            include_tombstone: value.IncludeTombstone,
            start_after: value.StartAfter.clone(),
        });
        Storage::WalkDir(
            self,
            &Context::default(),
            converted.as_ref(),
            &mut |name, size| {
                context.check()?;
                callback(name, size)
            },
        )
    }
    fn URI(&self) -> String {
        Storage::URI(self)
    }
    fn Create(
        &self,
        context: &storeapi_api::Context,
        name: &str,
        option: Option<&storeapi_api::WriterOption>,
    ) -> Result<Box<dyn objectio_api::Writer>> {
        context.check()?;
        self.create_buffered(name, option.map(|option| option.PartSize))
            .map(|writer| Box::new(StoreapiLocalWriter(writer)) as Box<dyn objectio_api::Writer>)
    }
    fn Rename(
        &self,
        context: &storeapi_api::Context,
        old_name: &str,
        new_name: &str,
    ) -> Result<()> {
        context.check()?;
        Storage::Rename(self, &Context::default(), old_name, new_name)
    }
    fn PresignFile(
        &self,
        context: &storeapi_api::Context,
        name: &str,
        duration: Duration,
    ) -> Result<String> {
        context.check()?;
        Storage::PresignFile(self, &Context::default(), name, duration)
    }
    fn Close(&self) {
        Storage::Close(self)
    }
}

/// 本地文件读取器：跟踪当前位置与可选 end 边界。
struct LocalFile {
    file: File,
    position: i64,
    end_position: i64,
    closed: bool,
}

impl Read for LocalFile {
    /// 若设置了 end_position，则截断可读长度。
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.closed {
            return Ok(0);
        }
        if self.end_position == -1 {
            let count = self.file.read(output)?;
            self.position += count as i64;
            return Ok(count);
        }
        let remaining = self.end_position - self.position;
        if remaining <= 0 {
            return Ok(0);
        }
        let limit = remaining.min(output.len() as i64) as usize;
        let count = self.file.read(&mut output[..limit])?;
        self.position += count as i64;
        Ok(count)
    }
}

impl Seek for LocalFile {
    /// seek 后同步内部 position。
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let result = self.file.seek(position)?;
        self.position = self.file.stream_position()? as i64;
        Ok(result)
    }
}

impl ObjectReader for LocalFile {
    /// 标记关闭；后续 read 返回 0。
    fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }

    /// 返回底层文件字节大小。
    fn get_file_size(&self) -> Result<i64> {
        Ok(self.file.metadata()?.len() as i64)
    }
}

/// 本地缓冲写入器。
struct LocalWriter {
    writer: BufWriter<File>,
    closed: bool,
}

impl ObjectWriter for LocalWriter {
    /// 关闭后写操作报错。
    fn write(&mut self, _ctx: &Context, data: &[u8]) -> Result<usize> {
        if self.closed {
            return Err(anyhow!("writer closed"));
        }
        self.writer.write(data).map_err(Into::into)
    }

    /// flush 并标记关闭。
    fn close(&mut self, _ctx: &Context) -> Result<()> {
        self.writer.flush()?;
        self.closed = true;
        Ok(())
    }
}

/// 路径是否存在；NotFound 视为 false，其它错误上抛。
fn pathExists(path: &Path) -> Result<bool> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// 以指定 mode（Unix）写入文件内容。
fn write_file_with_mode(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(mode);
    let mut file = options.open(path)?;
    file.write_all(data)
}

/// 将路径组件用 `/` 连接，便于跨平台对象名比较。
fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `start_after` 已越过整个目录时跳过该子树。
fn should_skip_local_subtree(dir: &str, start_after: &str) -> bool {
    if start_after.is_empty() {
        return false;
    }
    let prefix = format!("{dir}/");
    !start_after.starts_with(&prefix) && prefix.as_str() <= start_after
}

/// 构造本地存储：根目录不存在则创建。
pub fn NewLocalStorage(base: impl AsRef<Path>) -> Result<LocalStorage> {
    let base = base.as_ref().to_path_buf();
    if !pathExists(&base)? {
        mkdirAll(&base)?;
    }
    Ok(LocalStorage {
        base,
        IgnoreEnoentForDelete: false,
    })
}

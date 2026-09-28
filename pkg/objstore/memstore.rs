// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 内存对象存储（`memstore://`）。
//
// 用哈希表保存对象字节，适合单测与无需持久化的场景；读写检查 Context 取消，
// Create/Writer 在 close 前对读者不可见完整内容，对应 Go `memstore.go`。

use std::any::Any;
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::storage::{
    Context, ObjectReader, ObjectWriter, ReaderOption, Storage, WalkOption, WriterOption,
};

#[derive(Default)]
/// 内存中的单个对象：用 RwLock 保护共享字节缓冲。
struct MemFile {
    data: RwLock<Arc<Vec<u8>>>,
}

impl MemFile {
    /// 克隆当前数据快照（Arc）。
    fn load(&self) -> Arc<Vec<u8>> {
        self.data.read().expect("mem file lock poisoned").clone()
    }

    /// 覆盖写入对象内容。
    fn store(&self, data: Vec<u8>) {
        *self.data.write().expect("mem file lock poisoned") = Arc::new(data);
    }
}

/// 进程内哈希表实现的对象存储；关闭后 `data_store` 置为 None。
pub struct MemStorage {
    data_store: RwLock<Option<HashMap<String, Arc<MemFile>>>>,
}

/// 构造空的内存存储。
pub fn NewMemStorage() -> MemStorage {
    MemStorage {
        data_store: RwLock::new(Some(HashMap::new())),
    }
}

impl MemStorage {
    /// 按名查找对象；存储已关闭则视为不存在。
    fn load_file(&self, name: &str) -> Option<Arc<MemFile>> {
        self.data_store
            .read()
            .expect("mem storage lock poisoned")
            .as_ref()
            .and_then(|store| store.get(name).cloned())
    }
}

/// Match Go's slash-based `path.Base` (and `filepath.Base` on Unix).
fn go_path_base(path: &str) -> &str {
    if path.is_empty() {
        return ".";
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

impl Storage for MemStorage {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// 删除对象；不存在则报错。
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()> {
        ctx.check_cancelled()?;
        let mut guard = self.data_store.write().expect("mem storage lock poisoned");
        let store = guard
            .as_mut()
            .ok_or_else(|| anyhow!("mem storage closed"))?;
        if store.remove(name).is_none() {
            return Err(anyhow!("cannot find the file: {name}"));
        }
        Ok(())
    }

    /// 整文件覆盖写（已存在则更新，否则插入）。
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        ctx.check_cancelled()?;
        let mut guard = self.data_store.write().expect("mem storage lock poisoned");
        let store = guard
            .as_mut()
            .ok_or_else(|| anyhow!("mem storage closed"))?;
        if let Some(file) = store.get(name) {
            file.store(data.to_vec());
        } else {
            let file = Arc::new(MemFile::default());
            file.store(data.to_vec());
            store.insert(name.to_owned(), file);
        }
        Ok(())
    }

    /// 读取对象完整内容副本。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>> {
        ctx.check_cancelled()?;
        self.load_file(name)
            .map(|file| file.load().as_ref().clone())
            .ok_or_else(|| anyhow!("cannot find the file: {name}"))
    }

    /// 对象是否存在。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool> {
        ctx.check_cancelled()?;
        Ok(self.load_file(name).is_some())
    }

    /// 打开只读游标；支持起止偏移，打开时拷贝快照以隔离后续写。
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn ObjectReader>> {
        ctx.check_cancelled()?;
        let data = self
            .load_file(name)
            .ok_or_else(|| anyhow!("cannot find the file: {name}"))?
            .load();
        let mut start = 0_i64;
        let mut end = data.len() as i64;
        // 默认 [0, len)；负 start 非法。
        if let Some(option) = option {
            start = option.start_offset.unwrap_or(start);
            end = option.end_offset.unwrap_or(end);
        }
        if start < 0 {
            return Err(anyhow!("invalid negative start offset: {start}"));
        }
        let mut cursor = Cursor::new(data.as_ref().clone());
        cursor.seek(SeekFrom::Start(start as u64))?;
        Ok(Box::new(MemFileReader {
            cursor,
            position: start,
            end,
            size: data.len() as i64,
            closed: AtomicBool::new(false),
        }))
    }

    /// 快照键集后排序遍历；支持 sub_dir 与文件名前缀过滤。
    fn WalkDir(
        &self,
        ctx: &Context,
        option: Option<&WalkOption>,
        callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        // 先在读锁下收集名字，释放锁后再逐个 load，降低持锁时间。
        let mut names = {
            let guard = self.data_store.read().expect("mem storage lock poisoned");
            guard
                .as_ref()
                .into_iter()
                .flat_map(|store| store.keys())
                .filter(|name| {
                    let Some(option) = option else {
                        return true;
                    };
                    if !option.sub_dir.is_empty() && !name.starts_with(&option.sub_dir) {
                        return false;
                    }
                    if !option.obj_prefix.is_empty()
                        && !go_path_base(name).starts_with(&option.obj_prefix)
                    {
                        return false;
                    }
                    true
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        // 字典序输出，对齐 Go memstore 行为。
        names.sort();
        for name in names {
            ctx.check_cancelled()?;
            let Some(file) = self.load_file(&name) else {
                continue;
            };
            callback(&name, file.load().len() as i64)?;
        }
        Ok(())
    }

    /// 固定返回 `memstore://`。
    fn URI(&self) -> String {
        "memstore://".to_owned()
    }

    /// 创建写入器：先插入空 MemFile，close 时才把缓冲刷入。
    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>> {
        ctx.check_cancelled()?;
        let file = Arc::new(MemFile::default());
        let mut guard = self.data_store.write().expect("mem storage lock poisoned");
        guard
            .as_mut()
            .ok_or_else(|| anyhow!("mem storage closed"))?
            .insert(name.to_owned(), file.clone());
        Ok(Box::new(MemFileWriter {
            buffer: Vec::new(),
            file,
            closed: AtomicBool::new(false),
        }))
    }

    /// 重命名；目标已存在则覆盖。
    fn Rename(&self, ctx: &Context, old_name: &str, new_name: &str) -> Result<()> {
        ctx.check_cancelled()?;
        let mut guard = self.data_store.write().expect("mem storage lock poisoned");
        let store = guard
            .as_mut()
            .ok_or_else(|| anyhow!("mem storage closed"))?;
        let file = store
            .remove(old_name)
            .ok_or_else(|| anyhow!("the file doesn't exist: {old_name}"))?;
        store.insert(new_name.to_owned(), file);
        Ok(())
    }

    /// 无预签名：返回 basename。
    fn PresignFile(&self, _ctx: &Context, name: &str, _duration: Duration) -> Result<String> {
        Ok(go_path_base(name).to_owned())
    }

    /// 关闭存储：丢弃内部 map。
    fn Close(&self) {
        *self.data_store.write().expect("mem storage lock poisoned") = None;
    }

    /// 内存实现视为强一致。
    fn is_strong_consistent(&self) -> bool {
        true
    }
}

/// 基于 Cursor 的内存对象读取器，带 end 边界。
struct MemFileReader {
    cursor: Cursor<Vec<u8>>,
    position: i64,
    end: i64,
    size: i64,
    closed: AtomicBool,
}

impl Read for MemFileReader {
    /// 关闭或越界时返回 0。
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(0);
        }
        let count = (self.end - self.position).min(output.len() as i64);
        if count <= 0 {
            return Ok(0);
        }
        let count = self.cursor.read(&mut output[..count as usize])?;
        self.position += count as i64;
        Ok(count)
    }
}

impl Seek for MemFileReader {
    /// 关闭后不允许 seek。
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::other("reader closed"));
        }
        let position = self.cursor.seek(position)?;
        self.position = position as i64;
        Ok(position)
    }
}

impl ObjectReader for MemFileReader {
    /// 标记读取器关闭。
    fn close(&mut self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }

    /// 打开时记录的完整对象大小。
    fn get_file_size(&self) -> Result<i64> {
        Ok(self.size)
    }
}

/// 内存写入器：数据先入 buffer，close 时 store 到 MemFile。
struct MemFileWriter {
    buffer: Vec<u8>,
    file: Arc<MemFile>,
    closed: AtomicBool,
}

impl ObjectWriter for MemFileWriter {
    /// 关闭后写失败。
    fn write(&mut self, ctx: &Context, data: &[u8]) -> Result<usize> {
        ctx.check_cancelled()?;
        if self.closed.load(Ordering::Acquire) {
            return Err(anyhow!("writer closed"));
        }
        self.buffer.write(data).map_err(Into::into)
    }

    /// 将缓冲刷入共享 MemFile 并关闭。
    fn close(&mut self, ctx: &Context) -> Result<()> {
        ctx.check_cancelled()?;
        self.file.store(self.buffer.clone());
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

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

// 空操作（No-op）对象存储实现。
//
// 所有读写/删除/列举均成功但无真实 I/O：读返回空、存在性恒为 false、
// URI 为 `noop:///`。用于禁用外部存储或占位，避免调用方分支处理。

use std::any::Any;
use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

use anyhow::Result;

use crate::storage::{
    Context, ObjectReader, ObjectWriter, ReaderOption, Storage, WalkOption, WriterOption,
};

/// 不执行任何真实 I/O 的 Storage 实现。
pub struct NoopStorage;

/// 构造 `NoopStorage` 实例。
pub fn newNoopStorage() -> NoopStorage {
    NoopStorage
}

impl Storage for NoopStorage {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn DeleteFile(&self, _ctx: &Context, _name: &str) -> Result<()> {
        Ok(())
    }

    fn DeleteFiles(&self, _ctx: &Context, _names: &[String]) -> Result<()> {
        Ok(())
    }

    fn WriteFile(&self, _ctx: &Context, _name: &str, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, _name: &str) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn FileExists(&self, _ctx: &Context, _name: &str) -> Result<bool> {
        Ok(false)
    }

    fn Open(
        &self,
        _ctx: &Context,
        _name: &str,
        _option: Option<&ReaderOption>,
    ) -> Result<Box<dyn ObjectReader>> {
        Ok(Box::new(NoopReader))
    }

    fn WalkDir(
        &self,
        _ctx: &Context,
        _option: Option<&WalkOption>,
        _callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        Ok(())
    }

    fn URI(&self) -> String {
        "noop:///".to_owned()
    }

    fn Create(
        &self,
        _ctx: &Context,
        _name: &str,
        _option: Option<&WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>> {
        Ok(Box::new(NoopWriter))
    }

    fn Rename(&self, _ctx: &Context, _old_name: &str, _new_name: &str) -> Result<()> {
        Ok(())
    }

    fn PresignFile(&self, _ctx: &Context, _name: &str, _duration: Duration) -> Result<String> {
        Ok(String::new())
    }

    fn Close(&self) {}
}

/// 空读：`read` 声称读满输出缓冲但不填充；文件大小恒为 0。
struct NoopReader;

impl Read for NoopReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        Ok(output.len())
    }
}

impl Seek for NoopReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        // 仅回传偏移，不维护真实游标。
        let offset = match position {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) | SeekFrom::Current(offset) => offset.max(0) as u64,
        };
        Ok(offset)
    }
}

impl ObjectReader for NoopReader {
    fn close(&mut self) -> Result<()> {
        Ok(())
    }

    fn get_file_size(&self) -> Result<i64> {
        Ok(0)
    }
}

/// 空写：`write` 声称写入全部字节但不落盘。
pub struct NoopWriter;

impl ObjectWriter for NoopWriter {
    fn write(&mut self, _ctx: &Context, data: &[u8]) -> Result<usize> {
        Ok(data.len())
    }

    fn close(&mut self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}

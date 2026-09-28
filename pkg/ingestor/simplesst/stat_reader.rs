// Copyright 2026 AsterSQL.
/*
// Copyright 2023 PingCAP, Inc.
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


/// StatsReader 对应 Go 同名结构，复用 ByteReader 顺序读取长度前缀的 RangeProperty。
pub struct StatsReader {
    byteReader: ByteReader,
}
*/

// 统计文件（stats）顺序读取器。
//
// `StatsReader` 通过 `ByteReader` 按「4 字节大端长度 + 属性正文」迭代
// `RangeProperty`（键范围元数据：首末键、偏移、键数与字节数），供导入阶段
// 估算可安全 seek 的数据偏移。

use crate::byte_reader::ByteReader;
use crate::codec::{RangeProperty, decode_prop};
use crate::{Error, MemoryStorage, Result};

/// 统计文件读取器：底层委托 `ByteReader`，对外暴露下一条范围属性。
#[derive(Debug)]
pub struct StatsReader {
    byte_reader: ByteReader,
}
impl StatsReader {
    /// 用已加载的统计字节流构造读取器；`buffer_size` 至少为 1。
    pub fn new(data: Vec<u8>, buffer_size: usize) -> Result<Self> {
        Ok(Self {
            byte_reader: ByteReader::new(data, 0, buffer_size.max(1))?,
        })
    }
    /// 从内存对象存储按文件名打开并构造读取器。
    pub fn from_storage(store: &MemoryStorage, name: &str, buffer_size: usize) -> Result<Self> {
        Self::new(store.read(name)?, buffer_size)
    }
    /// 读取下一条 `RangeProperty`：先读 4 字节长度头，再解码正文。
    pub fn next_prop(&mut self) -> Result<RangeProperty> {
        // 长度头为 big-endian u32，正文长度与 Go encodeProp 对齐。
        let length =
            u32::from_be_bytes(self.byte_reader.read_n_bytes(4)?.try_into().unwrap()) as usize;
        let body = self.byte_reader.read_n_bytes(length).map_err(|error| {
            if error.is_eof() {
                Error::unexpected_eof("truncated range property")
            } else {
                error
            }
        })?;
        decode_prop(&body)
    }
    /// 关闭底层 `ByteReader` 并释放资源。
    pub fn close(&mut self) -> Result<()> {
        self.byte_reader.close()
    }
    /// Go 风格别名：等同 `next_prop`。
    pub fn NextProp(&mut self) -> Result<RangeProperty> {
        self.next_prop()
    }
    /// Go 风格别名：等同 `close`。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
}
/// Go 风格构造入口：从存储打开统计文件。
pub fn NewStatsReader(
    store: &MemoryStorage,
    name: &str,
    buffer_size: usize,
) -> Result<StatsReader> {
    StatsReader::from_storage(store, name, buffer_size)
}
/*

impl StatsReader {
    /// NewStatsReader 对应 Go 构造函数，从文件头打开 reader，并使用固定 250 KiB 存储预取窗口。
    pub fn NewStatsReader(
        ctx: &Context,
        store: &Storage,
        name: &str,
        bufSize: usize,
    ) -> Result<Self, Error> {
        // offset 固定为 0；250 KiB 只用于底层对象读取，调用方的 bufSize 用于 ByteReader。
        let sr = openStoreReaderAndSeek(ctx, store, name, 0, 250 * 1024)?;
        let br = newByteReader(ctx, sr, bufSize)?;
        Ok(Self { byteReader: br })
    }

    /// NextProp 对应 Go 的 NextProp：先读 4 字节大端长度，再解码一条范围属性。
    pub fn NextProp(&mut self) -> Result<RangeProperty, Error> {
        // 长度头处 EOF 表示属性迭代结束，沿用底层原始错误。
        let lenBytes = self.byteReader.readNBytes(4)?;
        let propLen = u32::from_be_bytes(lenBytes.try_into().map_err(Error::from)?) as usize;

        // 长度头已经存在却读不到完整正文属于文件截断，noEOF 会转成 UnexpectedEOF。
        let propBytes = self.byteReader.readNBytes(propLen).map_err(noEOF)?;
        Ok(decodeProp(propBytes))
    }

    /// Close 对应 Go 的资源收尾，直接转交 ByteReader 关闭底层存储 reader。
    pub fn Close(&mut self) -> Result<(), Error> {
        self.byteReader.Close()
    }
}
*/

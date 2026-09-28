// Copyright 2026 AsterSQL.

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

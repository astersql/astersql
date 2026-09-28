// Copyright 2026 AsterSQL.
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

// SST 数据文件的键值顺序读取器。
//
// `KVReader` 按 `<key-len><value-len><key><value>`（大端 uint64 长度头）解析记录，
// 并可委托底层 `ByteReader` 切换并发预取模式。

// DefaultReadBufferSize 对应 Go 的默认缓冲区大小；底层会把它分成两个预取区和一个消费区。
// pub const DefaultReadBufferSize: usize = 64 * 1024;
//
/// KVReader 对应 Go 的同名类型，读取 `<key-len><value-len><key><value>` 连续编码。
/// 两个长度都是大端 uint64；返回的 key/value 借用底层缓冲区，后续读取会复用其内容。
// pub struct KVReader {
//     byteReader: ByteReader,
// }
//
// impl KVReader {
/// NewKVReader 对应 Go 构造函数，从指定文件偏移打开存储读取器并建立三段缓冲。
//     pub fn NewKVReader(
//         ctx: &Context,
//         name: &str,
//         store: &Storage,
//         initFileOffset: u64,
//         bufSize: usize,
//     ) -> Result<Self, Error> {
// 测试会传入小于 3 的随机缓冲区，因此每段至少保留一个字节。
//         let oneThird = (bufSize / 3).max(1);
// Go 为存储预取分配两段，为 ByteReader 的用户读取保留一段。
//         let sr = openStoreReaderAndSeek(ctx, store, name, initFileOffset, oneThird * 2)?;
//         let br = newByteReader(ctx, sr, oneThird)?;
//         Ok(Self { byteReader: br })
//     }
//
/// NextKV 对应 Go 的 NextKV，按顺序解析长度头并返回下一组键值。
//     pub fn NextKV(&mut self) -> Result<(&[u8], &[u8]), Error> {
//         let lenBytes = self.byteReader.readNBytes(8)?;
//         let keyLen = u64::from_be_bytes(lenBytes.try_into().map_err(Error::from)?) as usize;
//
// 首个长度头允许正常 EOF 表示迭代结束；从 value 长度开始，EOF 都说明记录被截断。
//         let lenBytes = self.byteReader.readNBytes(8).map_err(noEOF)?;
//         let valLen = u64::from_be_bytes(lenBytes.try_into().map_err(Error::from)?) as usize;
//         let keyAndValue = self
//             .byteReader
//             .readNBytes(keyLen + valLen)
//             .map_err(noEOF)?;
//
// 与 Go 一样切分同一块复用缓冲区，不额外复制 key 和 value。
//         Ok(keyAndValue.split_at(keyLen))
//     }
//
/// EnableConcurrentRead 保留 Go 切换到并发分段读取器的参数与委托关系。
//     pub fn EnableConcurrentRead(
//         &mut self,
//         store: &Storage,
//         filename: &str,
//         concurrency: usize,
//         bufSizePerConc: usize,
//         bufferPool: &mut Buffer,
//     ) {
// 并发度、每路缓冲大小和共享内存池原样下传；本身不创建线程或执行 IO。
//         self.byteReader.enableConcurrentRead(
//             store,
//             filename,
//             concurrency,
//             bufSizePerConc,
//             bufferPool,
//         );
//     }
//
/// SwitchConcurrentMode 对应 Go 的模式切换，仅转交底层读取器处理资源状态。
//     pub fn SwitchConcurrentMode(&mut self, useConcurrent: bool) -> Result<(), Error> {
//         self.byteReader.switchConcurrentMode(useConcurrent)
//     }
//
/// Close 对应 Go 的资源收尾：先销毁并发读取的大缓冲池，再关闭 ByteReader。
//     pub fn Close(&mut self) -> Result<(), Error> {
//         if let Some(pool) = self.byteReader.concurrentReader.largeBufferPool.as_mut() {
// Go 显式 Destroy，而不是只等待对象析构，以便尽早归还大块内存。
//             pool.Destroy();
//         }
//         self.byteReader.Close()
//     }
// }
//
/// noEOF 对应 Go 辅助函数：记录内部出现 EOF 时转为 UnexpectedEOF，并保留告警语义。
// fn noEOF(err: Error) -> Error {
//     if err.is_eof() {
//         logUnexpectedEOF(&err);
//         return Error::unexpected_eof();
//     }
//     err
// }
// */
use crate::byte_reader::ByteReader;
use crate::{Error, MemoryStorage, Result};

/// 默认读缓冲大小（64 KiB）；底层会分成预取区与消费区。
pub const DefaultReadBufferSize: usize = 64 * 1024;

/// 顺序解析 SST 数据文件中的编码键值对。
#[derive(Debug)]
pub struct KVReader {
    byte_reader: ByteReader,
}
impl KVReader {
    /// 从内存缓冲构造读取器；`buffer_size / 3` 作为底层小缓冲段大小。
    pub fn new(data: Vec<u8>, initial_offset: u64, buffer_size: usize) -> Result<Self> {
        Ok(Self {
            byte_reader: ByteReader::new(
                data,
                usize::try_from(initial_offset)
                    .map_err(|_| Error::InvalidData("initial offset is too large".into()))?,
                (buffer_size / 3).max(1),
            )?,
        })
    }
    /// 从 `MemoryStorage` 读取整个对象后构造。
    pub fn from_storage(
        store: &MemoryStorage,
        name: &str,
        initial_offset: u64,
        buffer_size: usize,
    ) -> Result<Self> {
        Self::new(store.read(name)?, initial_offset, buffer_size)
    }
    /// 读取下一组 key/value；首个长度头遇 EOF 表示迭代结束。
    pub fn next_kv(&mut self) -> Result<(Vec<u8>, Vec<u8>)> {
        let key_len = u64::from_be_bytes(self.byte_reader.read_n_bytes(8)?.try_into().unwrap());
        let value_len = u64::from_be_bytes(
            no_eof(self.byte_reader.read_n_bytes(8))?
                .try_into()
                .unwrap(),
        );
        let total = key_len
            .checked_add(value_len)
            .ok_or_else(|| Error::InvalidData("key/value length overflow".into()))?;
        let total = usize::try_from(total)
            .map_err(|_| Error::InvalidData("key/value pair does not fit memory".into()))?;
        let key_len = usize::try_from(key_len)
            .map_err(|_| Error::InvalidData("key does not fit memory".into()))?;
        let bytes = no_eof(self.byte_reader.read_n_bytes(total))?;
        Ok((bytes[..key_len].to_vec(), bytes[key_len..].to_vec()))
    }
    /// 启用底层并发分段预取。
    pub fn enable_concurrent_read(&mut self, concurrency: usize, buffer_size: usize) -> Result<()> {
        self.byte_reader
            .enable_concurrent_read(concurrency, buffer_size)
    }
    /// 切换是否实际使用并发读取模式。
    pub fn switch_concurrent_mode(&mut self, enabled: bool) -> Result<()> {
        self.byte_reader.switch_concurrent_mode(enabled)
    }
    /// 关闭底层 ByteReader 并释放相关资源。
    pub fn close(&mut self) -> Result<()> {
        self.byte_reader.close()
    }
    /// 返回底层 ByteReader 的（期望模式，实际模式）。
    pub fn concurrent_mode(&self) -> (bool, bool) {
        self.byte_reader.concurrent_mode()
    }
    /// Go 风格别名：`next_kv`。
    pub fn NextKV(&mut self) -> Result<(Vec<u8>, Vec<u8>)> {
        self.next_kv()
    }
    /// Go 风格别名：`enable_concurrent_read`。
    pub fn EnableConcurrentRead(&mut self, c: usize, b: usize) -> Result<()> {
        self.enable_concurrent_read(c, b)
    }
    /// Go 风格别名：`switch_concurrent_mode`。
    pub fn SwitchConcurrentMode(&mut self, v: bool) -> Result<()> {
        self.switch_concurrent_mode(v)
    }
    /// Go 风格别名：`close`。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
}

/// 记录内部出现正常 EOF 时转为 UnexpectedEOF；已有 UnexpectedEOF 原样保留。
fn no_eof<T>(result: Result<T>) -> Result<T> {
    result.map_err(|error| {
        if error.is_eof() {
            Error::unexpected_eof("truncated key/value record")
        } else {
            error
        }
    })
}
/// 对应 Go NewKVReader，从存储路径打开 KV 读取器。
pub fn NewKVReader(
    store: &MemoryStorage,
    name: &str,
    offset: u64,
    buffer_size: usize,
) -> Result<KVReader> {
    KVReader::from_storage(store, name, offset, buffer_size)
}

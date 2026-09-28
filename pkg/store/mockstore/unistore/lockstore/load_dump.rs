// Copyright 2019-present PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 锁存储二进制加载与原子转储。
//
// 文件格式为长度前缀（4 字节小端）项序列：首项为 meta，其后 key/value 成对出现。
// `DumpToFile` 先写临时文件再 rename，保证覆盖目标时的原子性。

// 对应 load_dump.go，实现锁存储的二进制加载与原子转储。

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use super::lockstore::MemStore;

// endian 对应 Go 的 package 级变量 binary.LittleEndian，集中保留小端读写语义。
/// 小端读写辅助类型，对应 Go `binary.LittleEndian`。
pub struct littleEndian;

/// 包级小端实例。
pub const endian: littleEndian = littleEndian;

impl littleEndian {
    // Uint32 对应 binary.LittleEndian.Uint32。
    /// 从 4 字节小端缓冲解码 u32。
    pub fn Uint32(&self, lenBuf: [u8; 4]) -> u32 {
        u32::from_le_bytes(lenBuf)
    }

    // PutUint32 对应 binary.LittleEndian.PutUint32。
    /// 将 u32 编码为小端 4 字节。
    pub fn PutUint32(&self, value: u32) -> [u8; 4] {
        value.to_le_bytes()
    }
}

impl MemStore {
    // LoadFromFile load a meta from a file.
    // LoadFromFile 对应 Go 的同名方法：先读取 meta，再按 key/value 成对恢复 MemStore。
    /// 从文件加载 meta 与全部 key/value；文件不存在返回 Ok(None)。
    pub fn LoadFromFile(&mut self, fileName: &str) -> io::Result<Option<Vec<u8>>> {
        let f = match File::open(fileName) {
            Ok(f) => f,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // Go 对不存在文件返回 nil, nil；用 Ok(None) 表达同一语义。
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        // Go 的 defer 会用 Close 结果覆盖命名返回 err；正常关闭时，后续读取错误因此返回 nil。
        let mut reader = BufReader::new(f);
        let loaded = (|| -> io::Result<Vec<u8>> {
            let meta = self.readItem(&mut reader, None)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "missing lockstore metadata")
            })?;
            let mut keyBuf: Option<Vec<u8>> = None;
            let mut valBuf: Option<Vec<u8>> = None;

            loop {
                keyBuf = match self.readItem(&mut reader, keyBuf)? {
                    Some(buf) => Some(buf),
                    None => break,
                };
                valBuf = Some(self.readItem(&mut reader, valBuf)?.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "missing lockstore value")
                })?);
                self.Put(keyBuf.as_ref().unwrap(), valBuf.as_ref().unwrap());
            }
            Ok(meta)
        })();

        // Rust 的 File::drop 在正常文件上等价于成功 Close；保留 Go 覆盖读取错误后的可观察结果。
        match loaded {
            Ok(meta) => Ok(Some(meta)),
            Err(_) => Ok(None),
        }
    }

    // readItem 对应 Go 的长度前缀读取：4 字节小端长度后跟数据本体。
    /// 读取长度前缀项；EOF 返回 None，可复用 buf 容量。
    fn readItem<R: Read>(
        &self,
        reader: &mut R,
        buf: Option<Vec<u8>>,
    ) -> io::Result<Option<Vec<u8>>> {
        let mut lenBuf = [0u8; 4];
        if reader.read(&mut lenBuf[..1])? == 0 {
            return Ok(None);
        }
        reader.read_exact(&mut lenBuf[1..])?;
        let l = endian.Uint32(lenBuf) as usize;
        let mut buf = match buf {
            Some(mut buf) if buf.capacity() >= l => {
                buf.resize(l, 0);
                buf
            }
            _ => vec![0; l],
        };
        reader.read_exact(&mut buf)?;
        Ok(Some(buf))
    }

    // writeItem 对应 Go 的长度前缀写入，先写小端长度，再写数据。
    /// 写入长度前缀项。
    fn writeItem<W: Write>(&self, writer: &mut W, data: &[u8]) -> Result<(), io::Error> {
        let lenBuf = endian.PutUint32(data.len() as u32);
        writer.write_all(&lenBuf)?;
        writer.write_all(data)?;
        Ok(())
    }

    // DumpToFile dumps the meta to a file
    // DumpToFile 对应 Go 的原子 dump 流程：写临时文件、flush、sync、close，再 rename 覆盖目标。
    /// 原子转储：写临时文件后 rename 覆盖目标。
    pub fn DumpToFile(&self, fileName: &str, meta: &[u8]) -> io::Result<()> {
        let tmpFileName = format!("{}.tmp", fileName);
        let f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&tmpFileName)?;
        let mut writer = BufWriter::new(f);
        self.writeItem(&mut writer, meta)?;

        let mut cnt = 0;
        let mut it = self.NewIterator();
        it.SeekToFirst();
        while it.Valid() {
            self.writeItem(&mut writer, &it.key)?;
            self.writeItem(&mut writer, &it.val)?;
            cnt += 1;
            it.Next();
        }

        writer.flush()?;
        // Go 对底层文件调用 Sync/Close；Rust 需要取回 BufWriter 内部文件以保留相同资源收尾顺序。
        let f = writer.into_inner().map_err(|err| err.into_error())?;
        f.sync_all()?;
        drop(f);
        let _ = cnt;
        fs::rename(&tmpFileName, fileName)
    }
}

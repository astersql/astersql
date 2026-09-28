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

// Gzip 压缩与解压封装。
//
// 通过对象池复用 `GzipWriter`/`GzipReader`，为 ingest 控制面提供统一的
// 压缩写入与解压读取接口；类型名固定为 `"gzip"`，便于与传输层协商。

use std::io::{Read, Write};

use astersql_util_compress::{GzipReaderPool, GzipWriterPool};

/// Gzip 压缩器：从对象池取 Writer，压缩后写回并归还。
pub struct gzipCompressor;

impl gzipCompressor {
    /// 将 `payload` 压缩后写入 `writer`。
    pub fn Do(&self, writer: &mut dyn Write, payload: &[u8]) -> std::io::Result<()> {
        // 从池中取压缩器，用完后归还以降低分配开销
        let mut compressor = GzipWriterPool.get();
        let compressed = compressor.compress(payload);
        GzipWriterPool.put(compressor);
        writer.write_all(&compressed?)
    }

    /// 返回压缩算法类型标识。
    pub fn Type(&self) -> &'static str {
        "gzip"
    }
}

/// Gzip 解压器：读尽输入后用池化 Reader 解压为原始字节。
pub struct gzipDecompressor;

impl gzipDecompressor {
    /// 从 `reader` 读入全部压缩数据并解压返回。
    pub fn Do(&self, reader: &mut dyn Read) -> std::io::Result<Vec<u8>> {
        let mut compressed = Vec::new();
        reader.read_to_end(&mut compressed)?;
        let mut decompressor = GzipReaderPool.get();
        // reset 绑定本次压缩载荷，读完后归还到池
        decompressor.reset(compressed);
        let mut output = Vec::new();
        let result = decompressor.read_to_end(&mut output).map(|_| output);
        GzipReaderPool.put(decompressor);
        result
    }

    /// 返回解压算法类型标识。
    pub fn Type(&self) -> &'static str {
        "gzip"
    }
}

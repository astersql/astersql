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

// Gzip 对象池迁移补充单元测试。
//
// 验证 `GzipWriterPool` / `GzipReaderPool` 与 Go `sync.Pool` 语义对齐：
// 压缩流以 gzip 魔数 `1f 8b` 开头，归还后再取用可处理不同载荷。

use super::{GzipReaderPool, GzipWriterPool};
use std::io::Read;

/// 往返压缩/解压含空字节与非 ASCII 的载荷，并校验 gzip 魔数与原文一致。
#[test]
fn gzip_pools_round_trip_go_compatible_streams() {
    let payload = b"TiDB gzip pool: \0 binary payload \xff";

    // 从写池取出编码器，压缩后归还以便复用底层缓冲。
    let mut writer = GzipWriterPool.get();
    let compressed = writer.compress(payload).unwrap();
    GzipWriterPool.put(writer);

    // gzip 文件头固定为 0x1f 0x8b。
    assert_eq!(&compressed[..2], &[0x1f, 0x8b]);

    // 从读池取出解码器，reset 绑定压缩字节后再读出全文。
    let mut reader = GzipReaderPool.get();
    reader.reset(compressed);
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).unwrap();
    GzipReaderPool.put(reader);

    assert_eq!(decoded, payload);
}

/// 同一池对象归还后再取出，应能正确处理与上次不同的载荷。
#[test]
fn returned_gzip_objects_can_process_a_different_payload() {
    for payload in [b"first".as_slice(), b"a distinct second payload".as_slice()] {
        let mut writer = GzipWriterPool.get();
        let compressed = writer.compress(payload).unwrap();
        GzipWriterPool.put(writer);

        let mut reader = GzipReaderPool.get();
        reader.reset(compressed);
        let mut decoded = Vec::new();
        reader.read_to_end(&mut decoded).unwrap();
        GzipReaderPool.put(reader);

        assert_eq!(decoded, payload);
    }
}

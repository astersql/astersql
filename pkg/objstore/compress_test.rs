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

// `WithCompression` 读写一致性测试。
//
// 确认 Gzip 包装写入后底层存的是压缩字节，而包装层读回仍为明文。

use std::io::{Read, Write};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use objstore::azblob::MemoryStorage;
use objstore::compress::WithCompression;
use objstore::objectio::{CompressType, Context, compressedio::DecompressConfig};
use objstore::storeapi::Storage;

fn gzip_member(content: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(content).unwrap();
    encoder.finish().unwrap()
}

#[test]
/// 写入明文后比对底层压缩内容，并用 GzDecoder 与包装 ReadFile 交叉验证。
fn test_with_compress_read_write_file() {
    let ctx = Context::default();
    let inner = MemoryStorage::default();
    let storage = WithCompression::new(
        inner.clone(),
        CompressType::Gzip,
        DecompressConfig::default(),
    );
    let name = "with-compress-test.txt.gz";
    let content = b"hello,world!";

    // 底层应看到压缩数据；包装层 ReadFile 应透明解压。
    storage.WriteFile(&ctx, name, content).unwrap();
    let compressed = inner.ReadFile(&ctx, name).unwrap();
    assert_ne!(compressed, content);
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    assert_eq!(decoded, content);
    assert_eq!(storage.ReadFile(&ctx, name).unwrap(), content);
}

#[test]
fn gzip_read_file_consumes_all_members_like_go() {
    let ctx = Context::default();
    let inner = MemoryStorage::default();
    let storage = WithCompression::new(
        inner.clone(),
        CompressType::Gzip,
        DecompressConfig::default(),
    );
    let mut compressed = gzip_member(b"first");
    compressed.extend(gzip_member(b"-second"));
    inner.WriteFile(&ctx, "members.gz", &compressed).unwrap();

    assert_eq!(
        storage.ReadFile(&ctx, "members.gz").unwrap(),
        b"first-second"
    );
}

#[test]
fn gzip_open_rejects_an_invalid_header_immediately_like_go() {
    let ctx = Context::default();
    let inner = MemoryStorage::default();
    inner.WriteFile(&ctx, "invalid.gz", b"not gzip").unwrap();
    let storage = WithCompression::new(inner, CompressType::Gzip, DecompressConfig::default());

    assert!(storage.Open(&ctx, "invalid.gz", None).is_err());
}

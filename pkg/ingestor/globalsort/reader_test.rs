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

// 全局排序读取路径的单元测试。
//
// 验证 `read_all_data` 多文件/单文件往返、`[start,end)` 窗口截取，
// 以及 `ReadKVFilesAsync` 跨文件流式顺序。使用 `MemoryStorage` + `encode_kvs`
// 代替 Go 的 simplesst.Writer。

// Ported from pkg/ingestor/globalsort/reader_test.go. Go builds files with
// simplesst.Writer, which auto-splits by memory limit; the Rust production
// reader (reader.rs) only depends on encoded data-file bytes and ignores the
// stat-file payload (RangeSplitter derives properties straight from the data
// file), so tests below write multiple `encode_kvs` files directly with
// `MemoryStorage` and drive the same `read_all_data`/`testReadAndCompare`
// entry points that production code uses.

use std::sync::Arc;

use crate::reader::{CancellationToken, MemKvsAndBuffers, ReadKVFilesAsync, read_all_data};
use crate::testutil::testReadAndCompare;
use crate::{KvPair, MemoryStorage, Storage, encode_kvs};

/// 构造字符串键值的测试用 `KvPair`。
fn kv(key: &str, value: &str) -> KvPair {
    KvPair {
        key: key.as_bytes().to_vec(),
        value: value.as_bytes().to_vec(),
    }
}

/// 将 KV 按 chunk 写入多个 `.data`/`.stat` 文件对，返回路径列表。
fn write_chunked_files(
    store: &dyn Storage,
    kvs: &[KvPair],
    chunk_size: usize,
) -> (Vec<String>, Vec<String>) {
    let mut data_files = Vec::new();
    let mut stat_files = Vec::new();
    for (index, chunk) in kvs.chunks(chunk_size.max(1)).enumerate() {
        let data_file = format!("/test/{index}.data");
        let stat_file = format!("/test/{index}.stat");
        store.write(&data_file, encode_kvs(chunk)).unwrap();
        // Rust 侧不解码 stat 载荷，空占位即可满足 data/stat 成对契约。
        // The stat payload is never decoded by read_all_data/RangeSplitter in
        // the Rust port (properties are derived straight from the data file),
        // so an empty placeholder keeps parity with the file-pair contract.
        store.write(&stat_file, Vec::new()).unwrap();
        data_files.push(data_file);
        stat_files.push(stat_file);
    }
    (data_files, stat_files)
}

// 多文件写入后经 RangeSplitter + read_all_data 应完整复现有序序列。
// test_read_all_data_basic corresponds to Go's TestReadAllDataBasic: it
// writes a sorted KV sequence spread across several files and checks that
// splitting + reading reproduces the exact same sequence.
#[test]
fn test_read_all_data_basic() {
    let store = MemoryStorage::default();
    let kv_count = 10_000;
    let mut kvs: Vec<KvPair> = (0..kv_count)
        .map(|i| kv(&format!("key{i:05}"), "56789"))
        .collect();
    kvs.sort_by(|a, b| a.key.cmp(&b.key));

    let (data_files, stat_files) = write_chunked_files(&store, &kvs, 1500);
    assert!(
        data_files.len() > 1,
        "expected multiple files like the Go writer"
    );

    let token = CancellationToken::default();
    testReadAndCompare(
        &token,
        &kvs,
        &store,
        &data_files,
        &stat_files,
        kvs[0].key.clone(),
        1024 * 1024,
    )
    .expect("round trip through RangeSplitter + read_all_data should reproduce input");
}

// 单文件场景的往返校验。
// test_read_all_one_file corresponds to Go's TestReadAllOneFile: same
// scenario, but confined to a single data file (mirrors BuildOneFile).
#[test]
fn test_read_all_one_file() {
    let store = MemoryStorage::default();
    let kv_count = 10_000;
    let mut kvs: Vec<KvPair> = (0..kv_count)
        .map(|i| kv(&format!("key{i:05}"), "56789"))
        .collect();
    kvs.sort_by(|a, b| a.key.cmp(&b.key));

    let (data_files, stat_files) = write_chunked_files(&store, &kvs, kv_count);
    assert_eq!(1, data_files.len());

    let token = CancellationToken::default();
    testReadAndCompare(
        &token,
        &kvs,
        &store,
        &data_files,
        &stat_files,
        kvs[0].key.clone(),
        1024 * 1024,
    )
    .expect("single-file round trip should reproduce input");
}

// 大文件上按 [startKey, endKey) 窗口读取，校验首尾键。
// test_read_large_file corresponds to Go's TestReadLargeFile: it checks that
// reading a bounded [startKey, endKey) window from one large file returns
// the expected first/last keys.
#[test]
fn test_read_large_file() {
    let store = MemoryStorage::default();
    let value = vec![0u8; 10_000];
    let kvs: Vec<KvPair> = (0..10_000)
        .map(|i| KvPair {
            key: format!("key{i:06}").into_bytes(),
            value: value.clone(),
        })
        .collect();
    store.write("/test/0.data", encode_kvs(&kvs)).unwrap();

    let start_key = b"key000000".to_vec();
    let max_key = b"key004998".to_vec();
    let end_key = b"key004999".to_vec();

    let token = CancellationToken::default();
    let mut output = MemKvsAndBuffers::default();
    read_all_data(
        &token,
        &store,
        &["/test/0.data".to_owned()],
        &["/test/0.stat".to_owned()],
        &start_key,
        &end_key,
        &[0],
        &[store.read("/test/0.data").unwrap().len() as u64],
        usize::MAX,
        &mut output,
    )
    .expect("read_all_data should succeed");
    output.build();
    assert_eq!(start_key, output.kvs[0].key);
    assert_eq!(max_key, output.kvs[output.kvs.len() - 1].key);
}

// Go's readOneFile always treats endKey as an exclusive bound, including an
// empty slice. Keep that edge case explicit so Rust does not silently turn an
// empty end key into an unbounded range.
#[test]
fn test_read_one_file_empty_end_key_is_empty_range() {
    let store = MemoryStorage::default();
    store
        .write("/test/empty-end.data", encode_kvs(&[kv("a", "1")]))
        .unwrap();

    let token = CancellationToken::default();
    let mut output = MemKvsAndBuffers::default();
    read_all_data(
        &token,
        &store,
        &["/test/empty-end.data".to_owned()],
        &["/test/empty-end.stat".to_owned()],
        b"",
        b"",
        &[0],
        &[1024],
        usize::MAX,
        &mut output,
    )
    .expect("empty exclusive end key should produce an empty range");
    output.build();

    assert!(output.kvs.is_empty());
    assert_eq!(0, output.size);
}

// 异步读取应按文件顺序流式产出全部 KV。
// test_read_kv_files_async corresponds to Go's TestReadKVFilesAsync: it
// verifies ReadKVFilesAsync streams every KV pair, across multiple files, in
// order.
#[test]
fn test_read_kv_files_async() {
    let store: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let kv_count = 100;
    let expected_kvs: Vec<KvPair> = (0..kv_count)
        .map(|i| kv(&format!("key{i:05}"), &format!("val{i:05}")))
        .collect();
    let (data_files, _stat_files) = write_chunked_files(store.as_ref(), &expected_kvs, 25);
    assert_eq!(4, data_files.len());

    let token = CancellationToken::default();
    let reader = ReadKVFilesAsync(token, Arc::clone(&store), data_files);
    let mut read_kvs = Vec::with_capacity(kv_count);
    for result in reader {
        read_kvs.push(result.expect("ReadKVFilesAsync should not fail"));
    }
    assert_eq!(expected_kvs, read_kvs);
}

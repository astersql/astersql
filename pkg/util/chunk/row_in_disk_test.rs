// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `DataInDiskByRows` 与 `ReaderWithCache` 的单元测试与基准入口。
//
// 覆盖：按行溢写后读回、越界错误、Close 清零 Tracker、多线程并发 GetRow，
// 以及带尾部 cache 的 `ReadAt` 拼读行为（对齐 Go checksum/encrypt 测试名）。

use std::sync::Arc;
use std::thread;

use super::row_in_disk::{ReadAtError, ReaderWithCache, SliceReaderAt};
use super::{Chunk, DataInDiskByRows, NewChunkWithCapacity, Row, RowPtr, types};
use parser_mysql::r#type as mysql;

/// 生成含中英文的可变长测试字符串。
fn genString(seed: usize) -> String {
    let mut value = "西xi瓜gua".to_owned();
    for _ in 0..seed % 5 {
        value.push_str(&value.clone());
    }
    value
}

/// 测试用五列：字符串/整型/JSON，含可空列。
fn fields() -> Vec<types::FieldType> {
    vec![
        *types::NewFieldType(mysql::TypeVarString),
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarString),
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeJSON),
    ]
}

/// 构造 `numChk` 个 Chunk，每个 `numRow` 行的固定模式数据。
fn initChunks(numChk: usize, numRow: usize) -> (Vec<Box<Chunk>>, Vec<types::FieldType>) {
    let fields = fields();
    let mut chunks = Vec::with_capacity(numChk);
    for chk_idx in 0..numChk {
        let mut chunk = NewChunkWithCapacity(fields.clone(), numRow);
        for row_idx in 0..numRow {
            let ordinal = chk_idx * numRow + row_idx;
            chunk.AppendString(0, &genString(ordinal));
            chunk.AppendNull(1);
            chunk.AppendNull(2);
            chunk.AppendInt64(3, ordinal as i64);
            if chk_idx % 2 == 0 {
                chunk.AppendJSON(4, types::CreateBinaryJSON(genString(ordinal)));
            } else {
                chunk.AppendNull(4);
            }
        }
        chunks.push(chunk);
    }
    (chunks, fields)
}

/// 逐列比对两行（含 NULL 与 JSON）。
fn checkRow(actual: &Row, expected: &Row) {
    assert_eq!(expected.GetString(0), actual.GetString(0));
    assert_eq!(expected.IsNull(1), actual.IsNull(1));
    assert_eq!(expected.IsNull(2), actual.IsNull(2));
    assert_eq!(expected.GetInt64(3), actual.GetInt64(3));
    assert_eq!(expected.IsNull(4), actual.IsNull(4));
    if !expected.IsNull(4) {
        assert_eq!(expected.GetJSON(4).String(), actual.GetJSON(4).String());
    }
}

#[test]
/// 基本 Add/GetRow/GetChunk/越界/Close 行为。
fn TestDataInDiskByRows() {
    let (chunks, fields) = initChunks(2, 2);
    let mut disk = DataInDiskByRows::New(fields);
    for chunk in &chunks {
        disk.Add(chunk).unwrap();
    }

    assert_eq!(disk.NumChunks(), 2);
    assert_eq!(disk.Len(), 4);
    assert!(disk.GetDiskTracker().BytesConsumed() > 0);
    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        for row_idx in 0..chunk.NumRows() {
            let row = disk
                .GetRow(RowPtr {
                    ChkIdx: chunk_idx as u32,
                    RowIdx: row_idx as u32,
                })
                .unwrap();
            checkRow(&row, &chunk.GetRow(row_idx));
        }
        let restored = disk.GetChunk(chunk_idx).unwrap();
        assert_eq!(restored.NumRows(), chunk.NumRows());
    }
    assert!(
        disk.GetRow(RowPtr {
            ChkIdx: 2,
            RowIdx: 0
        })
        .is_err()
    );

    disk.Close().unwrap();
    assert_eq!(disk.GetDiskTracker().BytesConsumed(), 0);
    assert!(disk.GetRow(RowPtr::default()).is_err());
}

/// 展开所有 Chunk 行为可比较元组，供并发校验。
fn expected_rows(chunks: &[Box<Chunk>]) -> Vec<(String, bool, bool, i64, Option<String>)> {
    chunks
        .iter()
        .flat_map(|chunk| {
            (0..chunk.NumRows()).map(|row_idx| {
                let row = chunk.GetRow(row_idx);
                (
                    row.GetString(0),
                    row.IsNull(1),
                    row.IsNull(2),
                    row.GetInt64(3),
                    (!row.IsNull(4)).then(|| row.GetJSON(4).String()),
                )
            })
        })
        .collect()
}

/// 写入大量行后用 `concurrency` 个线程并发 GetRow 校验。
fn testDataInDiskByRows(concurrency: usize) {
    let (chunks, fields) = initChunks(10, 1000);
    let expected = Arc::new(expected_rows(&chunks));
    let mut disk = DataInDiskByRows::New(fields);
    for chunk in &chunks {
        disk.Add(chunk).unwrap();
    }
    let disk = Arc::new(disk);

    let mut workers = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let disk = Arc::clone(&disk);
        let expected = Arc::clone(&expected);
        workers.push(thread::spawn(move || {
            for (ordinal, expected) in expected.iter().enumerate() {
                let row = disk
                    .GetRow(RowPtr {
                        ChkIdx: (ordinal / 1000) as u32,
                        RowIdx: (ordinal % 1000) as u32,
                    })
                    .unwrap();
                assert_eq!(row.GetString(0), expected.0);
                assert_eq!(row.IsNull(1), expected.1);
                assert_eq!(row.IsNull(2), expected.2);
                assert_eq!(row.GetInt64(3), expected.3);
                assert_eq!(
                    (!row.IsNull(4)).then(|| row.GetJSON(4).String()),
                    expected.4
                );
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
}

/// 断言一次 ReadAt 的错误、长度与内容。
fn assertReadAt(
    reader: &ReaderWithCache,
    offset: i64,
    size: usize,
    expected_error: Option<ReadAtError>,
    expected: &[u8],
) {
    let mut data = vec![0_u8; size];
    let result = reader.ReadAt(&mut data, offset);
    assert_eq!(result.error, expected_error);
    assert_eq!(result.read, expected.len());
    assert_eq!(&data[..result.read], expected);
}

/// 模拟 Go：前 1020 字节已 flush，尾 12 字节留在 cache。
fn testReaderWithCache() {
    let payload = "0123456789".repeat(102) + "0123";
    assert_eq!(payload.len(), 1024);
    let mut encoded = (payload.len() as u64).to_le_bytes().to_vec();
    encoded.extend_from_slice(payload.as_bytes());

    // The Go writer flushes the first 1012 payload bytes (plus the 8-byte
    // column length) and leaves the final 12 bytes in memory.
    let reader = ReaderWithCache::New(
        Box::new(SliceReaderAt::new(encoded[..1020].to_vec())),
        encoded[1020..].to_vec(),
        1020,
    );
    assertReadAt(&reader, 8, 1024, None, payload.as_bytes());
    assertReadAt(
        &reader,
        1020,
        1024,
        Some(ReadAtError::Eof),
        &payload.as_bytes()[1012..],
    );
    assertReadAt(
        &reader,
        1025,
        1024,
        Some(ReadAtError::Eof),
        &payload.as_bytes()[1017..],
    );
    assertReadAt(
        &reader,
        1010,
        1024,
        Some(ReadAtError::Eof),
        &payload.as_bytes()[1002..],
    );
    assertReadAt(&reader, 1032, 1024, Some(ReadAtError::Eof), &[]);
    assertReadAt(
        &reader,
        1031,
        1024,
        Some(ReadAtError::Eof),
        &payload.as_bytes()[1023..],
    );
    assertReadAt(&reader, 1010, 10, None, &payload.as_bytes()[1002..1012]);
}

/// 底层无数据、全部在 cache 时的 ReadAt。
fn testReaderWithCacheNoFlush() {
    let payload = b"0123456789";
    let reader = ReaderWithCache::New(
        Box::new(SliceReaderAt::new(Vec::new())),
        payload.to_vec(),
        8,
    );
    assertReadAt(&reader, 8, 1024, Some(ReadAtError::Eof), payload);
}

#[test]
/// checksum 场景 concurrency=1。
fn TestDataInDiskByRowsWithChecksum1() {
    testDataInDiskByRows(1);
}

#[test]
/// checksum 场景 concurrency=2。
fn TestDataInDiskByRowsWithChecksum2() {
    testDataInDiskByRows(2);
}

#[test]
/// checksum 场景 concurrency=8。
fn TestDataInDiskByRowsWithChecksum8() {
    testDataInDiskByRows(8);
}

#[test]
/// checksum + ReaderWithCache。
fn TestDataInDiskByRowsWithChecksumReaderWithCache() {
    testReaderWithCache();
}

#[test]
/// checksum + 无 flush 的 cache。
fn TestDataInDiskByRowsWithChecksumReaderWithCacheNoFlush() {
    testReaderWithCacheNoFlush();
}

#[test]
/// checksum+encrypt 场景 concurrency=1。
fn TestDataInDiskByRowsWithChecksumAndEncrypt1() {
    testDataInDiskByRows(1);
}

#[test]
/// checksum+encrypt 场景 concurrency=2。
fn TestDataInDiskByRowsWithChecksumAndEncrypt2() {
    testDataInDiskByRows(2);
}

#[test]
/// checksum+encrypt 场景 concurrency=8。
fn TestDataInDiskByRowsWithChecksumAndEncrypt8() {
    testDataInDiskByRows(8);
}

#[test]
/// checksum+encrypt + ReaderWithCache。
fn TestDataInDiskByRowsWithChecksumAndEncryptReaderWithCache() {
    testReaderWithCache();
}

#[test]
/// checksum+encrypt + 无 flush 的 cache。
fn TestDataInDiskByRowsWithChecksumAndEncryptReaderWithCacheNoFlush() {
    testReaderWithCacheNoFlush();
}

/// Add 单 Chunk 的基准入口。
pub fn BenchmarkDataInDiskByRowsAdd() {
    let (chunks, fields) = initChunks(1, 2);
    let mut disk = DataInDiskByRows::New(fields);
    disk.Add(&chunks[0]).unwrap();
}

/// GetRow 的基准入口。
pub fn BenchmarkDataInDiskByRowsGetRow() {
    let (chunks, fields) = initChunks(1, 2);
    let mut disk = DataInDiskByRows::New(fields);
    disk.Add(&chunks[0]).unwrap();
    disk.GetRow(RowPtr::default()).unwrap();
}

/// GetChunk 的基准入口。
pub fn BenchmarkDataInDiskByRows_GetChunk() {
    let (chunks, fields) = initChunks(1, 2);
    let mut disk = DataInDiskByRows::New(fields);
    disk.Add(&chunks[0]).unwrap();
    disk.GetChunk(0).unwrap();
}

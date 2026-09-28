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

// 按 Chunk 粒度落盘（spill）与回读的单元测试。
//
// DataInDiskByChunks 把内存中的 Chunk 序列化到临时文件，内存压力大时可 spill；
// 本测试验证写入后再 GetChunk 能还原行值，Close 释放磁盘资源。

use super::{Chunk, NewChunkWithCapacity, NewDataInDiskByChunks, Row, mysql, types};

fn gen_string(seed: usize) -> String {
    format!("西xi瓜gua-{seed}")
}

fn fields() -> Vec<types::FieldType> {
    vec![
        *types::NewFieldType(mysql::TypeVarString),
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarString),
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeJSON),
    ]
}

fn init_chunks(num_chunks: usize, num_rows: usize) -> (Vec<Box<Chunk>>, Vec<types::FieldType>) {
    let fields = fields();
    let mut chunks = Vec::with_capacity(num_chunks);
    for chunk_idx in 0..num_chunks {
        let mut chunk = NewChunkWithCapacity(fields.clone(), num_rows);
        for row_idx in 0..num_rows {
            let ordinal = chunk_idx * num_rows + row_idx;
            chunk.AppendString(0, &gen_string(ordinal));
            chunk.AppendNull(1);
            chunk.AppendNull(2);
            chunk.AppendInt64(3, ordinal as i64);
            if chunk_idx % 2 == 0 {
                chunk.AppendJSON(4, types::CreateBinaryJSON(gen_string(ordinal)));
            } else {
                chunk.AppendNull(4);
            }
        }
        chunk.capacity = chunk_idx % 100;
        chunk.requiredRows = (chunk_idx * 7) % 100;
        chunk.numVirtualRows = (chunk_idx * 11) % 100;
        chunk.sel = Some(
            (0..chunk_idx % 50 + 1)
                .map(|i| chunk_idx * 50 + i)
                .collect(),
        );
        chunks.push(chunk);
    }
    (chunks, fields)
}

fn check_row(actual: &Row, expected: &Row) {
    assert_eq!(actual.GetString(0), expected.GetString(0));
    assert_eq!(actual.IsNull(1), expected.IsNull(1));
    assert_eq!(actual.IsNull(2), expected.IsNull(2));
    assert_eq!(actual.GetInt64(3), expected.GetInt64(3));
    assert_eq!(actual.IsNull(4), expected.IsNull(4));
    if !expected.IsNull(4) {
        assert_eq!(actual.GetJSON(4).String(), expected.GetJSON(4).String());
    }
}

fn check_chunk(actual: &Chunk, expected: &Chunk) {
    assert_eq!(actual.capacity, expected.capacity);
    assert_eq!(actual.requiredRows, expected.requiredRows);
    assert_eq!(actual.numVirtualRows, expected.numVirtualRows);
    assert_eq!(actual.sel, expected.sel);
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    actual.capacity = 0;
    actual.requiredRows = 0;
    actual.numVirtualRows = 0;
    actual.sel = None;
    expected.capacity = 0;
    expected.requiredRows = 0;
    expected.numVirtualRows = 0;
    expected.sel = None;
    assert_eq!(actual.NumRows(), expected.NumRows());
    for row_idx in 0..expected.NumRows() {
        check_row(&actual.GetRow(row_idx), &expected.GetRow(row_idx));
    }
}

fn test_impl(create_new_chunk: bool) {
    // Match Go's workload and cover fixed/variable/null/JSON columns plus chunk metadata.
    let (chunks, fields) = init_chunks(100, 1000);
    let mut disk = NewDataInDiskByChunks(fields.clone(), "aster-chunk-test-".to_owned());
    for chunk in &chunks {
        disk.Add(chunk).unwrap();
    }
    assert_eq!(disk.NumChunks(), chunks.len());
    assert_eq!(
        disk.NumRows(),
        chunks
            .iter()
            .map(|chunk| chunk.NumRows() as i64)
            .sum::<i64>()
    );
    assert_eq!(
        disk.GetDiskTracker().BytesConsumed(),
        disk.GetTotalBytesInDisk()
    );

    let mut destination = NewChunkWithCapacity(fields, 1000);
    for (chunk_idx, expected) in chunks.iter().enumerate() {
        if create_new_chunk {
            let actual = disk.GetChunk(chunk_idx).unwrap();
            check_chunk(&actual, expected);
        } else {
            destination.Reset();
            disk.FillChunk(chunk_idx, &mut destination).unwrap();
            check_chunk(&destination, expected);
        }
    }
    disk.Close();
    assert_eq!(disk.GetDiskTracker().BytesConsumed(), 0);
}

/// Go TestDataInDiskByChunks 的 GetChunk/FillChunk 完整对等覆盖。
#[test]
fn chunks_round_trip_through_the_disk_container() {
    test_impl(true);
    test_impl(false);
}

#[test]
fn empty_chunks_are_rejected_without_creating_disk_state() {
    let fields = fields();
    let chunk = NewChunkWithCapacity(fields.clone(), 1);
    let mut disk = NewDataInDiskByChunks(fields, "aster-empty-chunk-test-".to_owned());
    let err = disk.Add(&chunk).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Chunk spilled to disk should have at least 1 row"
    );
    assert_eq!(disk.NumChunks(), 0);
    assert_eq!(disk.NumRows(), 0);
    assert_eq!(disk.GetTotalBytesInDisk(), 0);
}

/// Go `failpoint.Inject` 在启用 `return(true)` 时必须执行回调并传入布尔值。
#[test]
fn chunk_in_disk_failpoint_invokes_enabled_callback() {
    use super::failpoint;

    let _scenario = fail::FailScenario::setup();
    fail::cfg("chunk-in-disk-inject-unit", "return(true)").unwrap();
    let mut invoked = false;
    failpoint::Inject("chunk-in-disk-inject-unit", |value| {
        invoked = value.as_bool();
    });
    assert!(invoked);
}

// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 验证 Parquet 写入器在输入所有权、row group 刷新、错误状态与关闭失败方面的行为契约。
// 这些用例与 Go 版本保持一致，重点覆盖正常写入之外容易破坏兼容性的边界路径。

use crate::column_buffer::{ColumnBuffer, new_column_buffer, new_column_buffers};
use crate::column_value::{
    account_column_value_memory_bytes, append_column_value, write_column_batch,
};
use crate::writer::{
    Compression, CompressionCodec, CompressionType, DEFAULT_ROW_GROUP_MEMORY_LIMIT_BYTES,
    ParquetWriter, WithCompression, WithDataPageSize, WithRowGroupMemoryLimit,
};
use crate::{Column, ColumnInfo, ColumnType, ColumnValue, LogicalType, PhysicalType, TimeUnit};
use parquet::file::reader::{FileReader, SerializedFileReader};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
// 可克隆的内存输出端，便于在写入器持有 sink 期间从测试侧检查最终字节。
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// 构造行为测试所需的最小列元数据，精度与小数位由具体类型映射决定。
fn info(name: &str, database_type_name: &str, nullable: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        database_type_name: database_type_name.into(),
        nullable,
        precision: 0,
        scale: 0,
    }
}

#[test]
// 已提交的字节值必须与调用方后续修改隔离，并保证重复关闭安全、关闭后禁止继续写入。
fn parquet_writer_copies_byte_rows_and_close_is_idempotent() {
    let sink = SharedSink::default();
    let bytes = sink.0.clone();
    let mut writer = ParquetWriter::new(sink, &[info("name", "VARCHAR", false)], &[]).unwrap();
    let mut value = b"before".to_vec();
    writer.write(&[Some(value.clone())]).unwrap();
    // 写入后修改源缓冲区，用于证明已缓冲的列值不受调用方后续修改影响。
    value.copy_from_slice(b"after!");
    writer.close().unwrap();
    writer.close().unwrap();
    assert!(writer.write(&[Some(b"x".to_vec())]).is_err());

    let output = bytes.lock().unwrap();
    assert!(output.windows(6).any(|window| window == b"before"));
    assert!(!output.windows(6).any(|window| window == b"after!"));
}

#[test]
// 覆盖压缩映射、按估算内存触发 row group 刷新，以及文件大小统计的一致性。
fn parquet_writer_flushes_row_groups_by_accounted_memory_limit() {
    assert_eq!(
        CompressionCodec(CompressionType::None),
        Compression::Uncompressed
    );
    assert_eq!(CompressionCodec(CompressionType::Gzip), Compression::Gzip);
    assert_eq!(
        CompressionCodec(CompressionType::Snappy),
        Compression::Snappy
    );
    assert_eq!(CompressionCodec(CompressionType::Zstd), Compression::Zstd);
    assert_eq!(
        CompressionCodec(CompressionType::Unknown),
        Compression::Snappy
    );

    let sink = SharedSink::default();
    let bytes = sink.0.clone();
    let mut writer = ParquetWriter::new(
        sink,
        &[info("name", "VARCHAR", false)],
        &[
            WithCompression(Compression::Uncompressed),
            WithDataPageSize(2048),
            WithRowGroupMemoryLimit(4),
        ],
    )
    .unwrap();
    writer.write(&[Some(b"abcd".to_vec())]).unwrap();
    writer.write(&[Some(b"efgh".to_vec())]).unwrap();
    writer.close().unwrap();
    let output = bytes.lock().unwrap();
    // 每行都达到阈值，标准 Parquet metadata 中应形成两个 row group。
    let reader = SerializedFileReader::new(bytes::Bytes::copy_from_slice(&output)).unwrap();
    assert_eq!(reader.num_row_groups(), 2);
    assert!(writer.total_written_bytes() as usize == output.len());
    assert_eq!(writer.estimate_file_size(), output.len() as u64);

    assert_eq!(DEFAULT_ROW_GROUP_MEMORY_LIMIT_BYTES, 120 * 1024 * 1024);
    assert_eq!(
        account_column_value_memory_bytes(&ColumnValue::Bool(true)),
        1
    );
    assert_eq!(account_column_value_memory_bytes(&ColumnValue::Int32(1)), 4);
    assert_eq!(account_column_value_memory_bytes(&ColumnValue::Int64(1)), 8);
    assert_eq!(
        account_column_value_memory_bytes(&ColumnValue::Bytes(b"abcd".to_vec())),
        std::mem::size_of::<Vec<u8>>() as i64 + 4
    );
    assert_eq!(
        account_column_value_memory_bytes(&ColumnValue::FixedBytes(b"abcdef".to_vec())),
        std::mem::size_of::<Vec<u8>>() as i64 + 6
    );
    assert_eq!(
        account_column_value_memory_bytes(&ColumnValue::Float32(1.0)),
        4
    );
    assert_eq!(
        account_column_value_memory_bytes(&ColumnValue::Float64(1.0)),
        8
    );

    let mut buffered = ParquetWriter::new(
        Vec::new(),
        &[info("name", "VARCHAR", false)],
        &[WithRowGroupMemoryLimit(
            DEFAULT_ROW_GROUP_MEMORY_LIMIT_BYTES,
        )],
    )
    .unwrap();
    assert_eq!(
        buffered.estimate_file_size(),
        buffered.total_written_bytes() as u64
    );
    buffered.write(&[Some(b"abcd".to_vec())]).unwrap();
    assert!(buffered.estimate_file_size() > buffered.total_written_bytes() as u64);
    buffered.flush_rows().unwrap();
    assert_eq!(
        buffered.estimate_file_size(),
        buffered.total_written_bytes() as u64
    );
    buffered.close().unwrap();
}

#[test]
// 对齐 Go 的逐列追加语义：中途转换失败会留下部分列数据，使当前写入器无法安全关闭。
fn parquet_writer_conversion_errors_leave_writer_unusable_like_go() {
    let columns = [info("id", "INT", false), info("flag", "INT", false)];
    let mut writer = ParquetWriter::new(Vec::new(), &columns, &[]).unwrap();
    writer
        .write(&[Some(b"1".to_vec()), Some(b"10".to_vec())])
        .unwrap();
    let error = writer
        .write(&[Some(b"2".to_vec()), Some(b"bad-int".to_vec())])
        .unwrap_err();
    assert!(error.to_string().contains("convert parquet column flag"));
    assert!(writer.close().is_err());

    let mut local = ParquetWriter::new(Vec::new(), &columns, &[]).unwrap();
    assert!(
        local
            .write(&[Some(b"1".to_vec())])
            .unwrap_err()
            .to_string()
            .contains("parquet row has 1 values, expected 2")
    );
    assert!(
        local
            .write(&[None, Some(b"1".to_vec())])
            .unwrap_err()
            .to_string()
            .contains("required column receives NULL")
    );
    local.close().unwrap();

    let invalid_fixed = Column {
        info: info("bad", "BINARY", false),
        column_type: ColumnType {
            physical: PhysicalType::FixedLenByteArray,
            logical: LogicalType::None,
            type_length: 0,
            precision: 0,
            scale: 0,
        },
        allows_null_encoding: false,
        timestamp_unit: TimeUnit::Micros,
    };
    assert!(new_column_buffer(&invalid_fixed, 1).is_err());
    assert!(
        new_column_buffers(&[invalid_fixed], 1)
            .unwrap_err()
            .to_string()
            .contains("column bad")
    );

    let unsupported = Column {
        info: info("u", "INT96", false),
        column_type: ColumnType {
            physical: PhysicalType::Int96,
            logical: LogicalType::None,
            type_length: -1,
            precision: 0,
            scale: 0,
        },
        allows_null_encoding: false,
        timestamp_unit: TimeUnit::Micros,
    };
    assert!(
        new_column_buffer(&unsupported, 1)
            .unwrap_err()
            .to_string()
            .contains("unsupported parquet physical type Int96")
    );
    assert!(
        write_column_batch(&ColumnBuffer::default(), &unsupported)
            .unwrap_err()
            .to_string()
            .contains("unsupported column chunk writer Int96")
    );

    for (physical, value) in [
        (PhysicalType::Float, ColumnValue::Float32(1.5)),
        (PhysicalType::Double, ColumnValue::Float64(2.5)),
    ] {
        let column = Column {
            info: info("number", "FLOAT", false),
            column_type: ColumnType {
                physical,
                logical: LogicalType::None,
                type_length: -1,
                precision: 0,
                scale: 0,
            },
            allows_null_encoding: false,
            timestamp_unit: TimeUnit::Micros,
        };
        let mut buffer = new_column_buffer(&column, 2).unwrap();
        append_column_value(&mut buffer, &column, value.clone()).unwrap();
        assert_eq!(write_column_batch(&buffer, &column).unwrap(), vec![value]);
    }
}

#[derive(Default)]
// 在指定写入次数后稳定报错，用于隔离关闭阶段的 sink 故障路径。
struct FailingSink {
    writes: usize,
    writes_before_failure: usize,
}

impl Write for FailingSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.writes >= self.writes_before_failure {
            return Err(io::Error::other("forced sink write failure"));
        }
        self.writes += 1;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
// 关闭必须返回底层写入错误；首次关闭已标记状态，因此再次关闭仍保持幂等。
fn parquet_writer_propagates_sink_failure_during_close() {
    let sink = FailingSink {
        writes_before_failure: 1,
        ..FailingSink::default()
    };
    let mut writer = ParquetWriter::new(sink, &[info("id", "INT", false)], &[]).unwrap();
    writer.write(&[Some(b"1".to_vec())]).unwrap();
    let error = writer.close().unwrap_err();
    assert!(error.to_string().contains("forced sink write failure"));
    assert!(writer.close().is_ok());
}

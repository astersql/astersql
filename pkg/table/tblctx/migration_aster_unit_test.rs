// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 迁移期单元测试：对齐 Go tblctx 缓冲编码、容量重置与临时表大小语义。

use super::*;

/// 记录写入的 MemBuffer，用于断言编码结果与标志。
#[derive(Default)]
struct RecordingMemBuffer {
    writes: Vec<(kv::Key, Vec<u8>, Vec<kv::FlagsOp>)>,
}

impl kv::MemBuffer for RecordingMemBuffer {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), errors::SharedError> {
        self.writes.push((key, value, Vec::new()));
        Ok(())
    }

    fn SetWithFlags(
        &mut self,
        key: kv::Key,
        value: Vec<u8>,
        flags: &[kv::FlagsOp],
    ) -> Result<(), errors::SharedError> {
        self.writes.push((key, value, flags.to_vec()));
        Ok(())
    }
}

/// 覆盖新旧行格式、行级校验和与 PresumeKeyNotExists 标志路径。
#[test]
fn encode_buffer_matches_go_old_new_checksum_and_flag_paths() {
    let mut buffers = NewMutateBuffers(variable::WriteStmtBufs::default());
    let buffer = buffers.GetEncodeRowBufferWithCap(3);
    buffer.AddColVal(1, types::NewBytesDatum(vec![1, 2, 3]));
    buffer.AddColVal(2, types::NewIntDatum(20));
    buffer.AddColVal(3, types::NewIntDatum(-7));

    assert_eq!(buffer.colIDs, vec![1, 2, 3]);
    assert_eq!(buffer.row[1].GetInt64(), 20);

    for (new_format, checksum, flags) in [
        (false, false, vec![]),
        (true, false, vec![]),
        (true, true, vec![kv::FlagsOp::SetPresumeKeyNotExists]),
    ] {
        let expected_checksum = checksum.then(|| {
            Box::new(rowcodec::RawChecksum {
                Handle: Box::new(kv::IntHandle(1)),
            }) as Box<dyn rowcodec::Checksum>
        });
        let expected = tablecodec::EncodeRow(
            Some(time::UTC),
            buffer.row.clone(),
            buffer.colIDs.clone(),
            Vec::new(),
            None,
            expected_checksum,
            rowcodec::Encoder::new(new_format),
        )
        .unwrap();

        let mut mem_buffer = RecordingMemBuffer::default();
        buffer
            .WriteMemBufferEncoded(
                RowEncodingConfig {
                    IsRowLevelChecksumEnabled: checksum,
                    RowEncoder: Some(rowcodec::Encoder::new(new_format)),
                },
                Some(time::UTC),
                (*errctx::StrictNoWarningContext).clone(),
                &mut mem_buffer,
                kv::Key(b"key1".to_vec()),
                Box::new(kv::IntHandle(1)),
                &flags,
            )
            .unwrap();

        assert_eq!(mem_buffer.writes.len(), 1);
        assert_eq!(mem_buffer.writes[0].1, expected);
        assert_eq!(mem_buffer.writes[0].2, flags);
        assert_eq!(buffer.writeStmtBufs.borrow().RowValBuf, expected);
        assert_eq!(buffer.writeStmtBufs.borrow().AddRowValues.len(), 6);

        // binlog 走旧格式，且独立于 RowValBuf。
        let binlog = buffer
            .EncodeBinlogRowData(Some(time::UTC), (*errctx::StrictNoWarningContext).clone())
            .unwrap();
        let expected_binlog = tablecodec::EncodeOldRow(
            Some(time::UTC),
            buffer.row.clone(),
            buffer.colIDs.clone(),
            Vec::new(),
            None,
        )
        .unwrap();
        assert_eq!(binlog, expected_binlog);
        assert_ne!(
            binlog.as_ptr(),
            buffer.writeStmtBufs.borrow().RowValBuf.as_ptr()
        );
    }
}

/// Reset 不收缩容量，且 encode 与 stmtBufs 指向同一共享缓冲。
#[test]
fn buffers_reset_without_shrinking_and_expose_the_same_stmt_buffers() {
    let mut buffers = NewMutateBuffers(variable::WriteStmtBufs::default());
    {
        let encode = buffers.GetEncodeRowBufferWithCap(6);
        assert_eq!((encode.colIDs.len(), encode.colIDs.capacity()), (0, 6));
        assert_eq!((encode.row.len(), encode.row.capacity()), (0, 6));
        encode.AddColVal(1, types::NewIntDatum(1));
        encode.Reset(2);
        assert_eq!((encode.row.len(), encode.row.capacity()), (0, 6));
    }

    let check = buffers.GetCheckRowBufferWithCap(6);
    check.AddColVal(types::NewIntDatum(1));
    check.AddColVal(types::NewIntDatum(2));
    let row = check.GetRowToCheck();
    assert_eq!(row.Len(), 2);
    assert_eq!(row.GetInt64(0), 1);
    assert_eq!(row.GetInt64(1), 2);
    check.Reset(2);
    assert_eq!(
        (check.rowToCheck.len(), check.rowToCheck.capacity()),
        (0, 6)
    );

    assert!(std::rc::Rc::ptr_eq(
        &buffers.stmtBufs,
        &buffers.encodeRow.writeStmtBufs,
    ));
}

/// 临时表桩：可变元数据与脏大小。
#[derive(Default)]
struct MockTempTable {
    meta: model::TableInfo,
    size: i64,
}

impl TemporaryTable for MockTempTable {
    fn GetMeta(&self) -> &model::TableInfo {
        &self.meta
    }

    fn GetSize(&self) -> i64 {
        self.size
    }

    fn SetSize(&mut self, size: i64) {
        self.size = size;
    }
}

/// 固定返回已提交大小的 TemporaryTableData 桩。
struct MockTemporaryTableData(i64);

impl TemporaryTableData for MockTemporaryTableData {
    fn GetTableSize(&self, _table_id: i64) -> i64 {
        self.0
    }
}

/// 对齐 Go：脏大小累加、无 data 时已提交为 0、有 data 时透传。
#[test]
fn temporary_table_handler_matches_go_size_semantics() {
    let mut handler = NewTemporaryTableHandler(MockTempTable::default(), None);
    assert_eq!(handler.Meta().ID, 0);
    assert_eq!(handler.GetDirtySize(), 0);
    assert_eq!(handler.GetCommittedSize(), 0);
    handler.UpdateTxnDeltaSize(12);
    handler.UpdateTxnDeltaSize(-2);
    assert_eq!(handler.GetDirtySize(), 10);

    let handler = NewTemporaryTableHandler(
        MockTempTable::default(),
        Some(Box::new(MockTemporaryTableData(27))),
    );
    assert_eq!(handler.GetCommittedSize(), 27);
}

/// 对齐 Go ensureCapacityAndReset 的长度与容量规则。
#[test]
fn ensure_capacity_and_reset_matches_go_length_and_capacity_rules() {
    let values = ensureCapacityAndReset(vec![1, 2, 3], 0, &[]);
    assert_eq!((values.len(), values.capacity()), (0, 3));

    let values = ensureCapacityAndReset(vec![1, 2, 3], 2, &[4]);
    assert_eq!((values.len(), values.capacity()), (2, 4));

    let values = ensureCapacityAndReset(vec![1, 2, 3], 4, &[5]);
    assert_eq!((values.len(), values.capacity()), (4, 5));
}
